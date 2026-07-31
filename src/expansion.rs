use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mail_parser::{MessageParser, MimeHeaders};
use msg_parser::Outlook;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unrar::Archive;
use zip::ZipArchive;

use crate::error::{ConverterError, Result};
use crate::media::{MediaInspection, SignatureStatus, detect_media_type, validate_file_signature};
use crate::path_policy::sanitize_filename;

const TAMANHO_BUFFER: usize = 256 * 1024;
const LIMITE_PREFIXO: usize = 16 * 1024;
const LIMITE_CAUDA: usize = 32 * 1024;

/// Descreve a capacidade aplicada ao arquivo contêiner analisado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerCapability {
    /// O arquivo não é um contêiner que exija expansão.
    NotContainer,
    /// O contêiner foi expandido e todos os filhos permitidos foram materializados.
    Expanded,
    /// O formato foi preservado, mas não existe extrator habilitado.
    PreservedWithoutExtractor,
    /// A expansão foi desabilitada pela política do trabalho.
    Disabled,
    /// A expansão foi interrompida por limite de segurança.
    LimitReached,
    /// O contêiner foi preservado, porém ocorreu erro de leitura ou integridade.
    FailedPreserved,
}

/// Controla profundidade, volume, formatos e comportamento de segurança da expansão.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpansionPolicy {
    /// Habilita a expansão de e-mails e arquivos compactados.
    #[serde(rename = "enabled", default = "padrao_verdadeiro")]
    pub habilitada: bool,
    /// Limita quantos níveis pai-filho podem ser percorridos.
    #[serde(rename = "max_depth", default = "profundidade_padrao")]
    pub profundidade_maxima: usize,
    /// Limita a quantidade de arquivos extraídos por anexo de origem.
    #[serde(rename = "max_extracted_files", default = "quantidade_padrao")]
    pub quantidade_maxima_arquivos: usize,
    /// Limita a soma de bytes extraídos por anexo de origem.
    #[serde(rename = "max_total_extracted_bytes", default = "total_padrao_bytes")]
    pub total_maximo_extraido_bytes: u64,
    /// Limita o tamanho de cada arquivo filho.
    #[serde(
        rename = "max_single_extracted_bytes",
        default = "arquivo_padrao_bytes"
    )]
    pub maximo_por_arquivo_bytes: u64,
    /// Limita a relação entre tamanho descompactado e compactado.
    #[serde(rename = "max_compression_ratio", default = "taxa_padrao")]
    pub taxa_maxima_compressao: u64,
    /// Habilita expansão de mensagens RFC 5322, EML, MHT e MHTML.
    #[serde(rename = "expand_eml", default = "padrao_verdadeiro")]
    pub expandir_eml: bool,
    /// Habilita expansão de mensagens Outlook MSG e modelos OFT.
    #[serde(rename = "expand_msg", default = "padrao_verdadeiro")]
    pub expandir_msg: bool,
    /// Habilita expansão de arquivos ZIP.
    #[serde(rename = "expand_zip", default = "padrao_verdadeiro")]
    pub expandir_zip: bool,
    /// Habilita expansão de arquivos 7z.
    #[serde(rename = "expand_7z", default = "padrao_verdadeiro")]
    pub expandir_7z: bool,
    /// Habilita expansão de RAR por biblioteca nativa isolada neste módulo.
    #[serde(rename = "expand_rar", default = "padrao_verdadeiro")]
    pub expandir_rar: bool,
    /// Habilita expansão de anexos TNEF, normalmente transportados como `winmail.dat`.
    #[serde(rename = "expand_tnef", default = "padrao_verdadeiro")]
    pub expandir_tnef: bool,
    /// Determina se um erro de expansão deve bloquear o trabalho.
    #[serde(rename = "fail_on_container_error", default)]
    pub falhar_em_erro_container: bool,
    /// Força sincronização dos filhos antes da publicação atômica.
    #[serde(rename = "fsync_extracted_files", default = "padrao_verdadeiro")]
    pub sincronizar_arquivos_extraidos: bool,
}

/// Fornece limites conservadores para conteúdo não confiável e arquivos compactados.
impl Default for ExpansionPolicy {
    /// Cria limites padrão para profundidade, quantidade, tamanho e expansão.
    fn default() -> Self {
        Self {
            habilitada: true,
            profundidade_maxima: profundidade_padrao(),
            quantidade_maxima_arquivos: quantidade_padrao(),
            total_maximo_extraido_bytes: total_padrao_bytes(),
            maximo_por_arquivo_bytes: arquivo_padrao_bytes(),
            taxa_maxima_compressao: taxa_padrao(),
            expandir_eml: true,
            expandir_msg: true,
            expandir_zip: true,
            expandir_7z: true,
            expandir_rar: true,
            expandir_tnef: true,
            falhar_em_erro_container: false,
            sincronizar_arquivos_extraidos: true,
        }
    }
}

/// Registra um arquivo filho extraído e sua linhagem técnica.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpandedFile {
    /// Identificador determinístico dentro do resultado de expansão.
    #[serde(rename = "file_id")]
    pub identificador_arquivo: String,
    /// Identificador do arquivo ou anexo que originou este filho.
    #[serde(rename = "parent_file_id")]
    pub identificador_pai: String,
    /// Profundidade do filho, iniciando em 1.
    #[serde(rename = "depth")]
    pub profundidade: usize,
    /// Nome exatamente informado pelo contêiner de origem.
    #[serde(rename = "original_name")]
    pub nome_original: String,
    /// Nome sanitizado utilizado no sistema de arquivos.
    #[serde(rename = "stored_name")]
    pub nome_armazenado: String,
    /// Caminho relativo ao diretório de saída do trabalho.
    #[serde(rename = "relative_path")]
    pub caminho_relativo: String,
    /// Origem técnica: `email_attachment`, `archive_entry` ou `embedded_message`.
    #[serde(rename = "relationship_type")]
    pub tipo_relacionamento: String,
    /// Tamanho exato materializado.
    #[serde(rename = "size_bytes")]
    pub tamanho_bytes: u64,
    /// SHA-256 do conteúdo materializado.
    pub sha256: String,
    /// Classificação e confiança sobre o formato.
    #[serde(rename = "media_inspection")]
    pub inspecao_midia: MediaInspection,
    /// Capacidade aplicada caso o filho também seja um contêiner.
    #[serde(rename = "container_capability")]
    pub capacidade_container: ContainerCapability,
}

/// Representa uma ocorrência não bloqueante, mas relevante para auditoria.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpansionWarning {
    /// Código estável para filtros e alertas operacionais.
    #[serde(rename = "code")]
    pub codigo: String,
    /// Mensagem legível para suporte e investigação.
    #[serde(rename = "message")]
    pub mensagem: String,
    /// Arquivo relacionado ao aviso.
    #[serde(rename = "relative_path")]
    pub caminho_relativo: String,
    /// Profundidade em que a ocorrência foi encontrada.
    #[serde(rename = "depth")]
    pub profundidade: usize,
}

/// Consolida todos os filhos, limites e avisos produzidos pela expansão.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpansionResult {
    /// Capacidade aplicada ao arquivo raiz.
    #[serde(rename = "root_capability")]
    pub capacidade_raiz: ContainerCapability,
    /// Indica que nenhuma expansão esperada ficou pendente ou falhou.
    #[serde(rename = "complete")]
    pub completa: bool,
    /// Quantidade total de arquivos filhos materializados.
    #[serde(rename = "extracted_file_count")]
    pub quantidade_arquivos_extraidos: usize,
    /// Soma de bytes dos arquivos filhos materializados.
    #[serde(rename = "total_extracted_bytes")]
    pub total_extraido_bytes: u64,
    /// Arquivos filhos com hash, mídia e linhagem.
    #[serde(rename = "files")]
    pub arquivos: Vec<ExpandedFile>,
    /// Ocorrências de segurança, limite ou formato não expandido.
    #[serde(rename = "warnings")]
    pub avisos: Vec<ExpansionWarning>,
}

/// Mantém estado e orçamento durante uma árvore recursiva de expansão.
struct ContextoExpansao<'a> {
    raiz_saida: &'a Path,
    politica: &'a ExpansionPolicy,
    arquivos: Vec<ExpandedFile>,
    avisos: Vec<ExpansionWarning>,
    total_extraido: u64,
    sequencia: usize,
    completa: bool,
}

/// Representa o arquivo filho já publicado, antes de sua expansão recursiva.
struct FilhoMaterializado {
    identificador: String,
    caminho_absoluto: PathBuf,
    caminho_relativo: PathBuf,
    nome_armazenado: String,
    tamanho: u64,
    sha256: String,
    inspecao_midia: MediaInspection,
}

/// Expande recursivamente um arquivo raiz segundo uma política limitada e auditável.
///
/// O arquivo raiz nunca é removido. Filhos são publicados em diretórios adjacentes
/// terminados em `.contents`, com nomes sanitizados e prefixos determinísticos.
pub fn expand_containers(
    caminho_raiz: &Path,
    raiz_saida: &Path,
    nome_logico: &str,
    identificador_raiz: &str,
    politica: &ExpansionPolicy,
) -> Result<ExpansionResult> {
    let mut contexto = ContextoExpansao {
        raiz_saida,
        politica,
        arquivos: Vec::new(),
        avisos: Vec::new(),
        total_extraido: 0,
        sequencia: 0,
        completa: true,
    };

    let capacidade_raiz = if politica.habilitada {
        contexto.processar_container(caminho_raiz, nome_logico, identificador_raiz, 0)?
    } else if tipo_container(nome_logico).is_some() {
        ContainerCapability::Disabled
    } else {
        ContainerCapability::NotContainer
    };

    Ok(ExpansionResult {
        capacidade_raiz,
        completa: contexto.completa,
        quantidade_arquivos_extraidos: contexto.arquivos.len(),
        total_extraido_bytes: contexto.total_extraido,
        arquivos: contexto.arquivos,
        avisos: contexto.avisos,
    })
}

/// Implementa as operações recursivas sobre um único contexto de orçamento.
impl ContextoExpansao<'_> {
    /// Seleciona o extrator adequado para o nome do arquivo informado.
    fn processar_container(
        &mut self,
        caminho: &Path,
        nome_logico: &str,
        identificador_pai: &str,
        profundidade: usize,
    ) -> Result<ContainerCapability> {
        let Some(tipo) = tipo_container(nome_logico) else {
            return Ok(ContainerCapability::NotContainer);
        };
        if profundidade >= self.politica.profundidade_maxima {
            self.registrar_aviso(
                "MAX_DEPTH_REACHED",
                format!(
                    "profundidade máxima {} atingida; contêiner preservado sem nova expansão",
                    self.politica.profundidade_maxima
                ),
                caminho,
                profundidade,
            );
            return Ok(ContainerCapability::LimitReached);
        }

        let resultado = match tipo {
            "eml" if self.politica.expandir_eml => {
                self.expandir_email_mime(caminho, identificador_pai, profundidade)
            }
            "msg" if self.politica.expandir_msg => {
                self.expandir_email_outlook(caminho, identificador_pai, profundidade)
            }
            "zip" if self.politica.expandir_zip => {
                self.expandir_zip(caminho, identificador_pai, profundidade)
            }
            "7z" if self.politica.expandir_7z => {
                self.expandir_7z(caminho, identificador_pai, profundidade)
            }
            "rar" if self.politica.expandir_rar => {
                self.expandir_rar(caminho, identificador_pai, profundidade)
            }
            "tnef" if self.politica.expandir_tnef && arquivo_tem_assinatura_tnef(caminho)? => {
                self.expandir_tnef(caminho, identificador_pai, profundidade)
            }
            "tnef" => return Ok(ContainerCapability::NotContainer),
            _ => {
                self.registrar_aviso(
                    "EXTRACTOR_DISABLED_OR_UNAVAILABLE",
                    format!("extrator para {tipo} desabilitado ou não disponível"),
                    caminho,
                    profundidade,
                );
                return Ok(ContainerCapability::PreservedWithoutExtractor);
            }
        };

        match resultado {
            Ok(()) => Ok(ContainerCapability::Expanded),
            Err(erro) if self.politica.falhar_em_erro_container => Err(erro),
            Err(erro) => {
                self.registrar_aviso(
                    "CONTAINER_EXPANSION_FAILED",
                    erro.to_string(),
                    caminho,
                    profundidade,
                );
                Ok(ContainerCapability::FailedPreserved)
            }
        }
    }

    /// Extrai anexos de EML, MHT ou MHTML e preserva mensagens RFC 5322 aninhadas.
    fn expandir_email_mime(
        &mut self,
        caminho: &Path,
        identificador_pai: &str,
        profundidade: usize,
    ) -> Result<()> {
        let tamanho = fs::metadata(caminho)
            .map_err(|origem| ConverterError::io(caminho, origem))?
            .len();
        if tamanho > self.politica.maximo_por_arquivo_bytes {
            return Err(ConverterError::ExpansionLimit(format!(
                "e-mail de {} bytes excede o limite individual de {}",
                tamanho, self.politica.maximo_por_arquivo_bytes
            )));
        }
        let conteudo = fs::read(caminho).map_err(|origem| ConverterError::io(caminho, origem))?;
        let mensagem =
            MessageParser::default()
                .parse(&conteudo)
                .ok_or_else(|| ConverterError::Container {
                    caminho: caminho.to_path_buf(),
                    motivo: "mensagem MIME inválida ou não reconhecida".to_string(),
                })?;

        for (indice, parte) in mensagem.attachments().enumerate() {
            let eh_mensagem = parte.message().is_some();
            let nome_original = parte
                .attachment_name()
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if eh_mensagem {
                        format!("mensagem_aninhada_{:04}.eml", indice + 1)
                    } else {
                        format!("anexo_{:04}.bin", indice + 1)
                    }
                });
            let relacionamento = if eh_mensagem {
                "embedded_message"
            } else {
                "email_attachment"
            };
            self.materializar_e_processar(
                caminho,
                identificador_pai,
                profundidade + 1,
                &nome_original,
                relacionamento,
                Cursor::new(parte.contents()),
                Some(parte.len() as u64),
            )?;
        }
        Ok(())
    }

    /// Extrai anexos comuns e mensagens incorporadas de arquivos Outlook MSG ou OFT.
    fn expandir_email_outlook(
        &mut self,
        caminho: &Path,
        identificador_pai: &str,
        profundidade: usize,
    ) -> Result<()> {
        let mensagem = Outlook::from_path(caminho).map_err(|erro| ConverterError::Container {
            caminho: caminho.to_path_buf(),
            motivo: format!("falha ao interpretar mensagem Outlook: {erro}"),
        })?;

        for (indice, anexo) in mensagem.attachments.iter().enumerate() {
            let nome_candidato = if !anexo.long_file_name.trim().is_empty() {
                anexo.long_file_name.clone()
            } else if !anexo.file_name.trim().is_empty() {
                anexo.file_name.clone()
            } else if anexo.is_embedded_message() {
                format!("mensagem_aninhada_{:04}.msg", indice + 1)
            } else if !anexo.extension.trim().is_empty() {
                format!(
                    "anexo_{:04}{}",
                    indice + 1,
                    normalizar_extensao_outlook(&anexo.extension)
                )
            } else {
                format!("anexo_{:04}.bin", indice + 1)
            };
            let relacionamento = if anexo.is_embedded_message() {
                "embedded_message"
            } else {
                "email_attachment"
            };
            self.materializar_e_processar(
                caminho,
                identificador_pai,
                profundidade + 1,
                &nome_candidato,
                relacionamento,
                Cursor::new(&anexo.payload_bytes),
                Some(anexo.payload_bytes.len() as u64),
            )?;
        }
        Ok(())
    }

    /// Extrai entradas ZIP sem aceitar caminhos absolutos, travessia ou links simbólicos.
    fn expandir_zip(
        &mut self,
        caminho: &Path,
        identificador_pai: &str,
        profundidade: usize,
    ) -> Result<()> {
        let arquivo = File::open(caminho).map_err(|origem| ConverterError::io(caminho, origem))?;
        let mut pacote = ZipArchive::new(arquivo).map_err(|erro| ConverterError::Container {
            caminho: caminho.to_path_buf(),
            motivo: format!("ZIP inválido: {erro}"),
        })?;

        for indice in 0..pacote.len() {
            let mut entrada =
                pacote
                    .by_index(indice)
                    .map_err(|erro| ConverterError::Container {
                        caminho: caminho.to_path_buf(),
                        motivo: format!("não foi possível abrir a entrada ZIP {indice}: {erro}"),
                    })?;
            if entrada.is_dir() {
                continue;
            }
            if entrada.enclosed_name().is_none() {
                self.registrar_aviso(
                    "ARCHIVE_PATH_REJECTED",
                    format!("entrada ZIP insegura rejeitada: {}", entrada.name()),
                    caminho,
                    profundidade + 1,
                );
                continue;
            }
            if entrada
                .unix_mode()
                .is_some_and(|modo| modo & 0o170000 == 0o120000)
            {
                self.registrar_aviso(
                    "ARCHIVE_SYMLINK_REJECTED",
                    format!("link simbólico ZIP rejeitado: {}", entrada.name()),
                    caminho,
                    profundidade + 1,
                );
                continue;
            }
            if entrada.encrypted() {
                self.registrar_aviso(
                    "ENCRYPTED_ENTRY_PRESERVED",
                    format!("entrada ZIP criptografada não extraída: {}", entrada.name()),
                    caminho,
                    profundidade + 1,
                );
                continue;
            }
            self.validar_taxa_compressao(
                entrada.size(),
                entrada.compressed_size(),
                entrada.name(),
            )?;
            let nome_original = entrada.name().to_string();
            let tamanho_entrada = entrada.size();
            self.materializar_e_processar(
                caminho,
                identificador_pai,
                profundidade + 1,
                &nome_original,
                "archive_entry",
                &mut entrada,
                Some(tamanho_entrada),
            )?;
        }
        Ok(())
    }

    /// Extrai entradas 7z usando decodificadores Rust e os mesmos limites globais.
    fn expandir_7z(
        &mut self,
        caminho: &Path,
        identificador_pai: &str,
        profundidade: usize,
    ) -> Result<()> {
        let diretorio_referencia = caminho
            .parent()
            .unwrap_or(self.raiz_saida)
            .join(".sevenz-validation");
        let resultado = sevenz_rust2::decompress_file_with_extract_fn(
            caminho,
            &diretorio_referencia,
            |entrada, leitor, _destino_validado| {
                if entrada.is_directory() || !entrada.has_stream() {
                    return Ok(true);
                }
                if let Err(erro) = self.validar_taxa_compressao(
                    entrada.size,
                    entrada.compressed_size,
                    entrada.name(),
                ) {
                    return Err(std::io::Error::other(erro.to_string()).into());
                }
                self.materializar_e_processar(
                    caminho,
                    identificador_pai,
                    profundidade + 1,
                    entrada.name(),
                    "archive_entry",
                    leitor,
                    Some(entrada.size),
                )
                .map_err(|erro| std::io::Error::other(erro.to_string()))?;
                Ok(true)
            },
        );
        let _ = fs::remove_dir(&diretorio_referencia);
        resultado.map_err(|erro| ConverterError::Container {
            caminho: caminho.to_path_buf(),
            motivo: format!("falha ao expandir 7z: {erro}"),
        })
    }

    /// Extrai entradas RAR após validar tamanho, criptografia e caminhos declarados.
    ///
    /// Esta é a única rotina que depende de código nativo `unrar`; sua fronteira
    /// isolada facilita sandbox, substituição ou execução em processo separado.
    fn expandir_rar(
        &mut self,
        caminho: &Path,
        identificador_pai: &str,
        profundidade: usize,
    ) -> Result<()> {
        let mut pacote = Archive::new(caminho)
            .open_for_processing()
            .map_err(|erro| ConverterError::Container {
                caminho: caminho.to_path_buf(),
                motivo: format!("não foi possível abrir RAR: {erro}"),
            })?;

        while let Some(cabecalho) =
            pacote
                .read_header()
                .map_err(|erro| ConverterError::Container {
                    caminho: caminho.to_path_buf(),
                    motivo: format!("não foi possível ler cabeçalho RAR: {erro}"),
                })?
        {
            let entrada = cabecalho.entry();
            let nome_original = entrada.filename.to_string_lossy().to_string();
            let tamanho_descompactado = entrada.unpacked_size;
            if !entrada.is_file() {
                pacote = cabecalho.skip().map_err(|erro| ConverterError::Container {
                    caminho: caminho.to_path_buf(),
                    motivo: format!("não foi possível avançar no RAR: {erro}"),
                })?;
                continue;
            }
            if entrada.is_encrypted() {
                self.registrar_aviso(
                    "ENCRYPTED_ENTRY_PRESERVED",
                    format!("entrada RAR criptografada não extraída: {nome_original}"),
                    caminho,
                    profundidade + 1,
                );
                pacote = cabecalho.skip().map_err(|erro| ConverterError::Container {
                    caminho: caminho.to_path_buf(),
                    motivo: format!("não foi possível avançar no RAR: {erro}"),
                })?;
                continue;
            }
            self.validar_tamanho_declarado(entrada.unpacked_size, &nome_original)?;
            let (conteudo, restante) =
                cabecalho.read().map_err(|erro| ConverterError::Container {
                    caminho: caminho.to_path_buf(),
                    motivo: format!("não foi possível ler entrada RAR: {erro}"),
                })?;
            pacote = restante;
            self.materializar_e_processar(
                caminho,
                identificador_pai,
                profundidade + 1,
                &nome_original,
                "archive_entry",
                Cursor::new(conteudo),
                Some(tamanho_descompactado),
            )?;
        }
        Ok(())
    }

    /// Extrai anexos de contêineres TNEF e continua a análise de filhos recursivamente.
    fn expandir_tnef(
        &mut self,
        caminho: &Path,
        identificador_pai: &str,
        profundidade: usize,
    ) -> Result<()> {
        let tamanho = fs::metadata(caminho)
            .map_err(|origem| ConverterError::io(caminho, origem))?
            .len();
        self.validar_tamanho_declarado(tamanho, &caminho.display().to_string())?;
        let conteudo = fs::read(caminho).map_err(|origem| ConverterError::io(caminho, origem))?;
        let anexos =
            tnef::read_attachments(&conteudo).map_err(|erro| ConverterError::Container {
                caminho: caminho.to_path_buf(),
                motivo: format!("falha ao interpretar contêiner TNEF: {erro}"),
            })?;
        for (indice, anexo) in anexos.iter().enumerate() {
            let nome_original = anexo
                .transport_filename
                .as_deref()
                .filter(|nome| !nome.trim().is_empty())
                .unwrap_or(&anexo.title);
            let nome_original = if nome_original.trim().is_empty() {
                format!("anexo_tnef_{:04}.bin", indice + 1)
            } else {
                nome_original.to_string()
            };
            self.materializar_e_processar(
                caminho,
                identificador_pai,
                profundidade + 1,
                &nome_original,
                "email_attachment",
                Cursor::new(anexo.data),
                Some(anexo.data.len() as u64),
            )?;
        }
        Ok(())
    }

    /// Materializa um filho, registra integridade e continua a expansão recursiva.
    #[allow(clippy::too_many_arguments)]
    fn materializar_e_processar<R: Read>(
        &mut self,
        caminho_pai: &Path,
        identificador_pai: &str,
        profundidade: usize,
        nome_original: &str,
        tipo_relacionamento: &str,
        leitor: R,
        tamanho_declarado: Option<u64>,
    ) -> Result<()> {
        self.validar_novo_arquivo(tamanho_declarado, nome_original)?;
        self.sequencia += 1;
        let identificador = format!("file-{:06}", self.sequencia);
        let nome_base = sanitize_filename(nome_original, "attachment.bin");
        let nome_armazenado = format!("{:06}_{}", self.sequencia, nome_base);
        let diretorio_filhos = diretorio_filhos(caminho_pai)?;
        fs::create_dir_all(&diretorio_filhos)
            .map_err(|origem| ConverterError::io(&diretorio_filhos, origem))?;
        let caminho_saida = diretorio_filhos.join(&nome_armazenado);

        let filho = self.gravar_filho_atomico(
            leitor,
            &caminho_saida,
            &identificador,
            &nome_armazenado,
            profundidade,
            tamanho_declarado,
        )?;
        self.total_extraido = self
            .total_extraido
            .checked_add(filho.tamanho)
            .ok_or_else(|| {
                ConverterError::ExpansionLimit(
                    "estouro numérico no total de bytes extraídos".to_string(),
                )
            })?;

        let indice_registro = self.arquivos.len();
        self.arquivos.push(ExpandedFile {
            identificador_arquivo: filho.identificador.clone(),
            identificador_pai: identificador_pai.to_string(),
            profundidade,
            nome_original: nome_original.to_string(),
            nome_armazenado: filho.nome_armazenado.clone(),
            caminho_relativo: caminho_portavel(&filho.caminho_relativo),
            tipo_relacionamento: tipo_relacionamento.to_string(),
            tamanho_bytes: filho.tamanho,
            sha256: filho.sha256.clone(),
            inspecao_midia: filho.inspecao_midia.clone(),
            capacidade_container: ContainerCapability::NotContainer,
        });

        let capacidade = self.processar_container(
            &filho.caminho_absoluto,
            &filho.nome_armazenado,
            &filho.identificador,
            profundidade,
        )?;
        self.arquivos[indice_registro].capacidade_container = capacidade;
        Ok(())
    }

    /// Grava um arquivo filho em streaming, calcula hash e publica atomicamente.
    fn gravar_filho_atomico<R: Read>(
        &mut self,
        mut leitor: R,
        caminho_saida: &Path,
        identificador: &str,
        nome_armazenado: &str,
        profundidade: usize,
        tamanho_declarado: Option<u64>,
    ) -> Result<FilhoMaterializado> {
        let caminho_temporario = caminho_temporario(caminho_saida)?;
        let mut arquivo = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&caminho_temporario)
            .map_err(|origem| ConverterError::io(&caminho_temporario, origem))?;
        let mut resumo = Sha256::new();
        let mut buffer = vec![0_u8; TAMANHO_BUFFER];
        let mut tamanho = 0_u64;
        let mut prefixo = Vec::with_capacity(LIMITE_PREFIXO);
        let mut cauda = Vec::with_capacity(LIMITE_CAUDA);

        let resultado = (|| -> Result<()> {
            loop {
                let quantidade = leitor
                    .read(&mut buffer)
                    .map_err(|origem| ConverterError::io(caminho_saida, origem))?;
                if quantidade == 0 {
                    break;
                }
                tamanho = tamanho.checked_add(quantidade as u64).ok_or_else(|| {
                    ConverterError::ExpansionLimit(
                        "estouro numérico no tamanho do arquivo extraído".to_string(),
                    )
                })?;
                self.validar_tamanho_declarado(tamanho, nome_armazenado)?;
                if self
                    .total_extraido
                    .checked_add(tamanho)
                    .is_none_or(|total| total > self.politica.total_maximo_extraido_bytes)
                {
                    return Err(ConverterError::ExpansionLimit(format!(
                        "total extraído excederia {} bytes",
                        self.politica.total_maximo_extraido_bytes
                    )));
                }
                let trecho = &buffer[..quantidade];
                resumo.update(trecho);
                capturar_prefixo(&mut prefixo, trecho);
                capturar_cauda(&mut cauda, trecho);
                arquivo
                    .write_all(trecho)
                    .map_err(|origem| ConverterError::io(&caminho_temporario, origem))?;
            }
            arquivo
                .flush()
                .map_err(|origem| ConverterError::io(&caminho_temporario, origem))?;
            if self.politica.sincronizar_arquivos_extraidos {
                arquivo
                    .sync_all()
                    .map_err(|origem| ConverterError::io(&caminho_temporario, origem))?;
            }
            Ok(())
        })();

        if let Err(erro) = resultado {
            drop(arquivo);
            let _ = fs::remove_file(&caminho_temporario);
            return Err(erro);
        }
        drop(arquivo);

        if let Some(declarado) = tamanho_declarado
            && declarado != tamanho
        {
            let _ = fs::remove_file(&caminho_temporario);
            return Err(ConverterError::SizeMismatch {
                esperado: declarado,
                atual: tamanho,
            });
        }
        let sha256 = hex::encode(resumo.finalize());
        publicar_filho(&caminho_temporario, caminho_saida, tamanho, &sha256)?;

        let inspecao_midia =
            match validate_file_signature(nome_armazenado, &prefixo, &cauda, None, true) {
                Ok(inspecao) => inspecao,
                Err(erro) => {
                    self.completa = false;
                    self.avisos.push(ExpansionWarning {
                        codigo: "EXTRACTED_SIGNATURE_MISMATCH".to_string(),
                        mensagem: erro.to_string(),
                        caminho_relativo: caminho_portavel(
                            caminho_saida
                                .strip_prefix(self.raiz_saida)
                                .unwrap_or(caminho_saida),
                        ),
                        profundidade,
                    });
                    MediaInspection {
                        tipo_mime: detect_media_type(&prefixo)
                            .unwrap_or("application/octet-stream")
                            .to_string(),
                        extensao: Path::new(nome_armazenado)
                            .extension()
                            .and_then(|valor| valor.to_str())
                            .unwrap_or_default()
                            .to_ascii_lowercase(),
                        estado_assinatura: SignatureStatus::UnknownPreserved,
                        detectado_por_assinatura: detect_media_type(&prefixo).map(str::to_string),
                        aviso: Some(
                            "arquivo filho preservado apesar da divergência de assinatura"
                                .to_string(),
                        ),
                    }
                }
            };
        let caminho_relativo = caminho_saida
            .strip_prefix(self.raiz_saida)
            .map_err(|_| ConverterError::SourcePathEscape(caminho_saida.display().to_string()))?
            .to_path_buf();

        Ok(FilhoMaterializado {
            identificador: identificador.to_string(),
            caminho_absoluto: caminho_saida.to_path_buf(),
            caminho_relativo,
            nome_armazenado: nome_armazenado.to_string(),
            tamanho,
            sha256,
            inspecao_midia,
        })
    }

    /// Verifica contagem, tamanho individual e total antes de materializar um filho.
    fn validar_novo_arquivo(
        &self,
        tamanho_declarado: Option<u64>,
        nome_original: &str,
    ) -> Result<()> {
        if self.arquivos.len() >= self.politica.quantidade_maxima_arquivos {
            return Err(ConverterError::ExpansionLimit(format!(
                "quantidade máxima de {} arquivos atingida",
                self.politica.quantidade_maxima_arquivos
            )));
        }
        if let Some(tamanho) = tamanho_declarado {
            self.validar_tamanho_declarado(tamanho, nome_original)?;
            if self
                .total_extraido
                .checked_add(tamanho)
                .is_none_or(|total| total > self.politica.total_maximo_extraido_bytes)
            {
                return Err(ConverterError::ExpansionLimit(format!(
                    "arquivo {nome_original} excederia o total máximo de {} bytes",
                    self.politica.total_maximo_extraido_bytes
                )));
            }
        }
        Ok(())
    }

    /// Verifica se um tamanho individual respeita o limite configurado.
    fn validar_tamanho_declarado(&self, tamanho: u64, nome: &str) -> Result<()> {
        if tamanho > self.politica.maximo_por_arquivo_bytes {
            return Err(ConverterError::ExpansionLimit(format!(
                "arquivo {nome} possui {tamanho} bytes e excede o limite individual de {}",
                self.politica.maximo_por_arquivo_bytes
            )));
        }
        Ok(())
    }

    /// Rejeita taxas de expansão compatíveis com bombas de compactação.
    fn validar_taxa_compressao(
        &self,
        descompactado: u64,
        compactado: u64,
        nome: &str,
    ) -> Result<()> {
        self.validar_tamanho_declarado(descompactado, nome)?;
        if compactado == 0 {
            return Ok(());
        }
        if descompactado > compactado.saturating_mul(self.politica.taxa_maxima_compressao) {
            return Err(ConverterError::ExpansionLimit(format!(
                "entrada {nome} excede taxa máxima de compressão {}:1",
                self.politica.taxa_maxima_compressao
            )));
        }
        Ok(())
    }

    /// Registra uma ocorrência e marca a expansão como incompleta.
    fn registrar_aviso(
        &mut self,
        codigo: &str,
        mensagem: String,
        caminho: &Path,
        profundidade: usize,
    ) {
        self.completa = false;
        let relativo = caminho.strip_prefix(self.raiz_saida).unwrap_or(caminho);
        self.avisos.push(ExpansionWarning {
            codigo: codigo.to_string(),
            mensagem,
            caminho_relativo: caminho_portavel(relativo),
            profundidade,
        });
    }
}

/// Classifica somente contêineres que devem ser abertos para obter arquivos filhos.
///
/// Pacotes Office Open XML não são tratados como ZIP, pois seu conteúdo interno
/// faz parte do próprio documento e não representa anexos independentes.
fn tipo_container(nome: &str) -> Option<&'static str> {
    match Path::new(nome)
        .extension()
        .and_then(|valor| valor.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "eml" | "mht" | "mhtml" => Some("eml"),
        "msg" | "oft" => Some("msg"),
        "zip" => Some("zip"),
        "7z" => Some("7z"),
        "rar" => Some("rar"),
        "dat" => Some("tnef"),
        "pst" | "ost" | "mbox" | "mbx" | "dbx" | "olm" => Some("unsupported"),
        _ => None,
    }
}

/// Cria o diretório adjacente onde os filhos de um contêiner serão publicados.
fn diretorio_filhos(caminho_pai: &Path) -> Result<PathBuf> {
    let nome = caminho_pai
        .file_name()
        .and_then(|valor| valor.to_str())
        .ok_or_else(|| ConverterError::UnsafeFilename(caminho_pai.display().to_string()))?;
    Ok(caminho_pai.with_file_name(format!("{nome}.contents")))
}

/// Gera um nome temporário exclusivo no mesmo diretório do destino.
fn caminho_temporario(caminho_saida: &Path) -> Result<PathBuf> {
    let diretorio = caminho_saida
        .parent()
        .ok_or_else(|| ConverterError::UnsafeRelativePath(caminho_saida.display().to_string()))?;
    let nome = caminho_saida
        .file_name()
        .and_then(|valor| valor.to_str())
        .ok_or_else(|| ConverterError::UnsafeFilename(caminho_saida.display().to_string()))?;
    let instante = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for tentativa in 0..100_u32 {
        let candidato = diretorio.join(format!(
            ".{nome}.{}.{}.{}.part",
            std::process::id(),
            instante,
            tentativa
        ));
        if !candidato.exists() {
            return Ok(candidato);
        }
    }
    Err(ConverterError::Spool(
        "não foi possível reservar temporário para arquivo extraído".to_string(),
    ))
}

/// Publica o filho ou aceita repetição idempotente com mesmo tamanho e hash.
fn publicar_filho(temporario: &Path, destino: &Path, tamanho: u64, sha256: &str) -> Result<()> {
    if destino.exists() {
        let metadados =
            fs::metadata(destino).map_err(|origem| ConverterError::io(destino, origem))?;
        if metadados.is_file()
            && metadados.len() == tamanho
            && crate::decoder::sha256_file(destino)? == sha256
        {
            fs::remove_file(temporario).map_err(|origem| ConverterError::io(temporario, origem))?;
            return Ok(());
        }
        return Err(ConverterError::OutputConflict {
            caminho: destino.to_path_buf(),
        });
    }
    fs::rename(temporario, destino).map_err(|origem| ConverterError::io(destino, origem))
}

/// Mantém os bytes iniciais necessários para classificação de mídia.
fn capturar_prefixo(prefixo: &mut Vec<u8>, trecho: &[u8]) {
    if prefixo.len() < LIMITE_PREFIXO {
        let faltante = LIMITE_PREFIXO - prefixo.len();
        prefixo.extend_from_slice(&trecho[..trecho.len().min(faltante)]);
    }
}

/// Mantém uma janela dos bytes finais necessária para formatos com marcador de rodapé.
fn capturar_cauda(cauda: &mut Vec<u8>, trecho: &[u8]) {
    if trecho.len() >= LIMITE_CAUDA {
        cauda.clear();
        cauda.extend_from_slice(&trecho[trecho.len() - LIMITE_CAUDA..]);
    } else {
        let excesso = cauda
            .len()
            .saturating_add(trecho.len())
            .saturating_sub(LIMITE_CAUDA);
        if excesso > 0 {
            cauda.drain(..excesso);
        }
        cauda.extend_from_slice(trecho);
    }
}

/// Normaliza extensões Outlook para que sempre comecem com ponto.
fn normalizar_extensao_outlook(extensao: &str) -> String {
    let limpa = extensao.trim();
    if limpa.is_empty() {
        String::new()
    } else if limpa.starts_with('.') {
        limpa.to_string()
    } else {
        format!(".{limpa}")
    }
}

/// Converte separadores locais para `/` no contrato JSON multiplataforma.
fn caminho_portavel(caminho: &Path) -> String {
    caminho
        .components()
        .map(|componente| componente.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Confirma a assinatura TNEF antes de tratar um arquivo genérico `.dat` como e-mail.
fn arquivo_tem_assinatura_tnef(caminho: &Path) -> Result<bool> {
    let mut arquivo = File::open(caminho).map_err(|origem| ConverterError::io(caminho, origem))?;
    let mut assinatura = [0_u8; 4];
    match arquivo.read_exact(&mut assinatura) {
        Ok(()) => Ok(assinatura == [0x78, 0x9F, 0x3E, 0x22]),
        Err(origem) if origem.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(origem) => Err(ConverterError::io(caminho, origem)),
    }
}

/// Retorna a profundidade recursiva padrão.
fn profundidade_padrao() -> usize {
    10
}

/// Retorna o limite padrão de arquivos filhos por anexo raiz.
fn quantidade_padrao() -> usize {
    5_000
}

/// Retorna o limite padrão de dois GiB extraídos por anexo raiz.
fn total_padrao_bytes() -> u64 {
    2 * 1024 * 1024 * 1024
}

/// Retorna o limite padrão de 512 MiB por arquivo filho.
fn arquivo_padrao_bytes() -> u64 {
    512 * 1024 * 1024
}

/// Retorna a taxa máxima padrão de expansão para compactação.
fn taxa_padrao() -> u64 {
    200
}

/// Retorna `true` para recursos seguros habilitados por padrão.
fn padrao_verdadeiro() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirma que pacotes Office não são expandidos como arquivos ZIP comuns.
    #[test]
    fn nao_expande_pacotes_office() {
        assert_eq!(tipo_container("contrato.docx"), None);
        assert_eq!(tipo_container("planilha.xlsx"), None);
        assert_eq!(tipo_container("evidencias.zip"), Some("zip"));
    }

    /// Confirma que nomes Outlook recebem ponto antes da extensão.
    #[test]
    fn normaliza_extensao_msg() {
        assert_eq!(normalizar_extensao_outlook("pdf"), ".pdf");
        assert_eq!(normalizar_extensao_outlook(".png"), ".png");
    }

    /// Confirma que caminhos JSON usam separador portátil.
    #[test]
    fn serializa_caminho_portavel() {
        assert_eq!(
            caminho_portavel(Path::new("documentos").join("arquivo.pdf").as_path()),
            "documentos/arquivo.pdf"
        );
    }

    /// Confirma que caminhos relativos declarados continuam submetidos à política central.
    #[test]
    fn politica_de_caminho_rejeita_travessia() {
        assert!(crate::path_policy::validate_relative_path(Path::new("../segredo.txt")).is_err());
    }
}
