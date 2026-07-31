use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::read::DecoderReader;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{ConverterError, Result};
use crate::media::{MediaInspection, SignatureStatus, validate_file_signature};
use crate::path_policy::validate_filename;

const TAMANHO_BUFFER: usize = 256 * 1024;
const LIMITE_PREFIXO: usize = 16 * 1024;
const LIMITE_CAUDA: usize = 32 * 1024;
const LIMITE_PADRAO_BYTES: u64 = 512 * 1024 * 1024;
const LIMITE_CABECALHO_DATA_URI: usize = 8 * 1024;

/// Define o alfabeto utilizado pelo conteúdo Base64 recebido.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Base64Variant {
    /// Alfabeto Base64 convencional utilizado por JSON e MIME.
    #[default]
    Standard,
    /// Variante URL-safe que substitui `+` e `/` por `-` e `_`.
    UrlSafe,
}

/// Configura limites, verificações e garantias da reconstrução Base64.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodeOptions {
    /// Limite máximo de bytes após a decodificação.
    #[serde(rename = "max_decoded_bytes", default = "default_max_bytes")]
    pub maximo_bytes_decodificados: u64,
    /// Tamanho esperado informado pelo sistema de origem.
    #[serde(rename = "expected_size_bytes", default)]
    pub tamanho_esperado_bytes: Option<u64>,
    /// SHA-256 esperado informado pelo sistema de origem.
    #[serde(rename = "expected_sha256", default)]
    pub sha256_esperado: Option<String>,
    /// Tipo MIME esperado informado pelo sistema de origem.
    #[serde(rename = "expected_media_type", default)]
    pub tipo_mime_esperado: Option<String>,
    /// Habilita a validação de extensão, assinatura e estrutura básica.
    #[serde(rename = "validate_signature", default = "default_true")]
    pub validar_assinatura: bool,
    /// Preserva extensões fora do catálogo em vez de bloquear o anexo.
    #[serde(rename = "preserve_unknown_formats", default = "default_true")]
    pub preservar_formatos_desconhecidos: bool,
    /// Força sincronização do arquivo temporário antes da publicação atômica.
    #[serde(rename = "fsync_before_commit", default = "default_true")]
    pub sincronizar_antes_publicacao: bool,
    /// Seleciona o alfabeto Base64 aceito.
    #[serde(rename = "base64_variant", default)]
    pub variante_base64: Base64Variant,
    /// Permite o prefixo opcional `data:<mime>;base64,`.
    #[serde(rename = "allow_data_uri_prefix", default = "default_true")]
    pub permitir_prefixo_data_uri: bool,
}

/// Fornece opções seguras para conteúdo não confiável de até 512 MiB por arquivo.
impl Default for DecodeOptions {
    /// Cria a política padrão com integridade, durabilidade e preservação habilitadas.
    fn default() -> Self {
        Self {
            maximo_bytes_decodificados: LIMITE_PADRAO_BYTES,
            tamanho_esperado_bytes: None,
            sha256_esperado: None,
            tipo_mime_esperado: None,
            validar_assinatura: true,
            preservar_formatos_desconhecidos: true,
            sincronizar_antes_publicacao: true,
            variante_base64: Base64Variant::Standard,
            permitir_prefixo_data_uri: true,
        }
    }
}

/// Registra o resultado íntegro e auditável de uma reconstrução Base64.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodeResult {
    /// Caminho físico publicado após todas as verificações.
    #[serde(rename = "output_path")]
    pub caminho_saida: String,
    /// Quantidade exata de bytes reconstruídos.
    #[serde(rename = "decoded_size_bytes")]
    pub tamanho_decodificado_bytes: u64,
    /// Hash SHA-256 calculado sobre o arquivo reconstruído.
    pub sha256: String,
    /// Resultado detalhado da classificação e validação de mídia.
    #[serde(rename = "media_inspection")]
    pub inspecao_midia: MediaInspection,
    /// Indica que o arquivo já existia com conteúdo idêntico.
    #[serde(rename = "idempotent_existing_file")]
    pub arquivo_existente_idempotente: bool,
    /// Tempo total da reconstrução e publicação.
    #[serde(rename = "duration_ms")]
    pub duracao_ms: u128,
}

/// Reconstrói um arquivo a partir de um arquivo textual Base64 em modo streaming.
///
/// O conteúdo decodificado nunca é carregado integralmente na memória. A função
/// delega as mesmas verificações utilizadas por entradas via `stdin` ou rede.
pub fn decode_base64_to_file(
    caminho_entrada: &Path,
    caminho_saida: &Path,
    nome_logico: &str,
    opcoes: &DecodeOptions,
) -> Result<DecodeResult> {
    let entrada = File::open(caminho_entrada)
        .map_err(|origem| ConverterError::io(caminho_entrada, origem))?;
    decode_base64_reader_to_file(entrada, caminho_saida, nome_logico, opcoes)
}

/// Reconstrói um arquivo a partir de qualquer fonte que implemente `Read`.
///
/// A saída é escrita em arquivo temporário no mesmo diretório, validada por
/// tamanho, hash e assinatura e só então renomeada atomicamente para o destino.
pub fn decode_base64_reader_to_file<R: Read>(
    entrada: R,
    caminho_saida: &Path,
    nome_logico: &str,
    opcoes: &DecodeOptions,
) -> Result<DecodeResult> {
    validate_filename(nome_logico)?;
    validar_hash_esperado(opcoes.sha256_esperado.as_deref())?;
    let inicio = Instant::now();

    let diretorio_pai = caminho_saida
        .parent()
        .ok_or_else(|| ConverterError::UnsafeRelativePath(caminho_saida.display().to_string()))?;
    fs::create_dir_all(diretorio_pai)
        .map_err(|origem| ConverterError::io(diretorio_pai, origem))?;

    let (caminho_temporario, arquivo_temporario) = criar_arquivo_temporario(caminho_saida)?;
    let resultado = decodificar_para_temporario(
        entrada,
        arquivo_temporario,
        &caminho_temporario,
        caminho_saida,
        nome_logico,
        opcoes,
        inicio,
    );
    if resultado.is_err() {
        let _ = fs::remove_file(&caminho_temporario);
    }
    resultado
}

/// Executa a decodificação, calcula integridade e publica o arquivo temporário.
#[allow(clippy::too_many_arguments)]
fn decodificar_para_temporario<R: Read>(
    entrada: R,
    mut saida: File,
    caminho_temporario: &Path,
    caminho_saida: &Path,
    nome_logico: &str,
    opcoes: &DecodeOptions,
    inicio: Instant,
) -> Result<DecodeResult> {
    let configuracao =
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent);
    let alfabeto = match opcoes.variante_base64 {
        Base64Variant::Standard => &alphabet::STANDARD,
        Base64Variant::UrlSafe => &alphabet::URL_SAFE,
    };
    let motor = GeneralPurpose::new(alfabeto, configuracao);
    let entrada_normalizada =
        LeitorBase64Normalizado::new(entrada, opcoes.permitir_prefixo_data_uri);
    let mut decodificador = DecoderReader::new(entrada_normalizada, &motor);
    let mut buffer = vec![0_u8; TAMANHO_BUFFER];
    let mut resumo = Sha256::new();
    let mut tamanho_decodificado = 0_u64;
    let mut prefixo = Vec::with_capacity(LIMITE_PREFIXO);
    let mut cauda = Vec::with_capacity(LIMITE_CAUDA);

    loop {
        let quantidade = decodificador
            .read(&mut buffer)
            .map_err(|origem| ConverterError::InvalidBase64(origem.to_string()))?;
        if quantidade == 0 {
            break;
        }
        tamanho_decodificado = tamanho_decodificado.checked_add(quantidade as u64).ok_or(
            ConverterError::SizeLimitExceeded {
                limite_bytes: opcoes.maximo_bytes_decodificados,
            },
        )?;
        if tamanho_decodificado > opcoes.maximo_bytes_decodificados {
            return Err(ConverterError::SizeLimitExceeded {
                limite_bytes: opcoes.maximo_bytes_decodificados,
            });
        }

        let trecho = &buffer[..quantidade];
        resumo.update(trecho);
        capturar_prefixo(&mut prefixo, trecho);
        capturar_cauda(&mut cauda, trecho);
        saida
            .write_all(trecho)
            .map_err(|origem| ConverterError::io(caminho_temporario, origem))?;
    }

    saida
        .flush()
        .map_err(|origem| ConverterError::io(caminho_temporario, origem))?;
    if opcoes.sincronizar_antes_publicacao {
        saida
            .sync_all()
            .map_err(|origem| ConverterError::io(caminho_temporario, origem))?;
    }
    drop(saida);

    validar_tamanho_esperado(tamanho_decodificado, opcoes.tamanho_esperado_bytes)?;
    let sha256 = hex::encode(resumo.finalize());
    validar_hash_calculado(&sha256, opcoes.sha256_esperado.as_deref())?;

    let inspecao_midia = if opcoes.validar_assinatura {
        validate_file_signature(
            nome_logico,
            &prefixo,
            &cauda,
            opcoes.tipo_mime_esperado.as_deref(),
            opcoes.preservar_formatos_desconhecidos,
        )?
    } else {
        MediaInspection {
            tipo_mime: opcoes
                .tipo_mime_esperado
                .clone()
                .unwrap_or_else(|| "application/octet-stream".to_string()),
            extensao: Path::new(nome_logico)
                .extension()
                .and_then(|valor| valor.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase(),
            estado_assinatura: SignatureStatus::ValidationDisabled,
            detectado_por_assinatura: None,
            aviso: Some("validação de assinatura desabilitada pelo chamador".to_string()),
        }
    };

    let idempotente = publicar_arquivo_temporario(
        caminho_temporario,
        caminho_saida,
        tamanho_decodificado,
        &sha256,
    )?;
    Ok(DecodeResult {
        caminho_saida: caminho_saida.display().to_string(),
        tamanho_decodificado_bytes: tamanho_decodificado,
        sha256,
        inspecao_midia,
        arquivo_existente_idempotente: idempotente,
        duracao_ms: inicio.elapsed().as_millis(),
    })
}

/// Cria um arquivo temporário exclusivo no mesmo volume do destino.
///
/// Manter temporário e destino no mesmo diretório permite que a publicação por
/// renomeação preserve atomicidade nos sistemas de arquivos suportados.
fn criar_arquivo_temporario(caminho_saida: &Path) -> Result<(PathBuf, File)> {
    let diretorio_pai = caminho_saida
        .parent()
        .ok_or_else(|| ConverterError::UnsafeRelativePath(caminho_saida.display().to_string()))?;
    let nome_arquivo = caminho_saida
        .file_name()
        .and_then(|valor| valor.to_str())
        .ok_or_else(|| ConverterError::UnsafeFilename(caminho_saida.display().to_string()))?;
    let instante = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    for tentativa in 0..100_u32 {
        let caminho = diretorio_pai.join(format!(
            ".{nome_arquivo}.{}.{}.{}.part",
            std::process::id(),
            instante,
            tentativa
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&caminho)
        {
            Ok(arquivo) => return Ok((caminho, arquivo)),
            Err(origem) if origem.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(origem) => return Err(ConverterError::io(&caminho, origem)),
        }
    }
    Err(ConverterError::Spool(
        "não foi possível reservar um arquivo temporário exclusivo".to_string(),
    ))
}

/// Publica o temporário ou reconhece uma repetição idempotente.
///
/// Um destino existente só é aceito quando tamanho e SHA-256 são idênticos.
/// Conteúdo divergente nunca é sobrescrito silenciosamente.
fn publicar_arquivo_temporario(
    caminho_temporario: &Path,
    caminho_saida: &Path,
    tamanho_decodificado: u64,
    sha256_decodificado: &str,
) -> Result<bool> {
    if arquivo_existente_identico(caminho_saida, tamanho_decodificado, sha256_decodificado)? {
        fs::remove_file(caminho_temporario)
            .map_err(|origem| ConverterError::io(caminho_temporario, origem))?;
        return Ok(true);
    }
    if caminho_saida.exists() {
        return Err(ConverterError::OutputConflict {
            caminho: caminho_saida.to_path_buf(),
        });
    }

    match fs::rename(caminho_temporario, caminho_saida) {
        Ok(()) => Ok(false),
        Err(origem) if origem.kind() == io::ErrorKind::AlreadyExists => {
            if arquivo_existente_identico(caminho_saida, tamanho_decodificado, sha256_decodificado)?
            {
                fs::remove_file(caminho_temporario)
                    .map_err(|erro| ConverterError::io(caminho_temporario, erro))?;
                Ok(true)
            } else {
                Err(ConverterError::OutputConflict {
                    caminho: caminho_saida.to_path_buf(),
                })
            }
        }
        Err(origem) => Err(ConverterError::io(caminho_saida, origem)),
    }
}

/// Verifica se um destino existente possui exatamente o conteúdo esperado.
fn arquivo_existente_identico(
    caminho: &Path,
    tamanho_esperado: u64,
    sha256_esperado: &str,
) -> Result<bool> {
    if !caminho.exists() {
        return Ok(false);
    }
    let metadados = fs::metadata(caminho).map_err(|origem| ConverterError::io(caminho, origem))?;
    if !metadados.is_file() || metadados.len() != tamanho_esperado {
        return Ok(false);
    }
    Ok(sha256_file(caminho)? == sha256_esperado)
}

/// Calcula SHA-256 em streaming sobre um arquivo já materializado.
pub(crate) fn sha256_file(caminho: &Path) -> Result<String> {
    let mut arquivo = File::open(caminho).map_err(|origem| ConverterError::io(caminho, origem))?;
    let mut resumo = Sha256::new();
    let mut buffer = vec![0_u8; TAMANHO_BUFFER];
    loop {
        let quantidade = arquivo
            .read(&mut buffer)
            .map_err(|origem| ConverterError::io(caminho, origem))?;
        if quantidade == 0 {
            break;
        }
        resumo.update(&buffer[..quantidade]);
    }
    Ok(hex::encode(resumo.finalize()))
}

/// Mantém os primeiros bytes necessários para identificação de assinatura.
fn capturar_prefixo(prefixo: &mut Vec<u8>, trecho: &[u8]) {
    if prefixo.len() >= LIMITE_PREFIXO {
        return;
    }
    let faltante = LIMITE_PREFIXO - prefixo.len();
    prefixo.extend_from_slice(&trecho[..trecho.len().min(faltante)]);
}

/// Mantém uma janela deslizante dos bytes finais para validações de rodapé.
fn capturar_cauda(cauda: &mut Vec<u8>, trecho: &[u8]) {
    if trecho.len() >= LIMITE_CAUDA {
        cauda.clear();
        cauda.extend_from_slice(&trecho[trecho.len() - LIMITE_CAUDA..]);
        return;
    }
    let excesso = cauda
        .len()
        .saturating_add(trecho.len())
        .saturating_sub(LIMITE_CAUDA);
    if excesso > 0 {
        cauda.drain(..excesso);
    }
    cauda.extend_from_slice(trecho);
}

/// Valida a sintaxe do SHA-256 esperado antes de iniciar operações de I/O.
fn validar_hash_esperado(esperado: Option<&str>) -> Result<()> {
    if let Some(esperado) = esperado
        && (esperado.len() != 64
            || !esperado
                .chars()
                .all(|caractere| caractere.is_ascii_hexdigit()))
    {
        return Err(ConverterError::InvalidJob(
            "expected_sha256 deve conter 64 caracteres hexadecimais".to_string(),
        ));
    }
    Ok(())
}

/// Compara o tamanho reconstruído com o tamanho informado pela origem.
fn validar_tamanho_esperado(tamanho: u64, esperado: Option<u64>) -> Result<()> {
    if let Some(esperado) = esperado
        && esperado != tamanho
    {
        return Err(ConverterError::SizeMismatch {
            esperado,
            atual: tamanho,
        });
    }
    Ok(())
}

/// Compara o hash reconstruído com o hash informado pela origem.
fn validar_hash_calculado(calculado: &str, esperado: Option<&str>) -> Result<()> {
    if let Some(esperado) = esperado
        && !esperado.eq_ignore_ascii_case(calculado)
    {
        return Err(ConverterError::HashMismatch {
            esperado: esperado.to_ascii_lowercase(),
            atual: calculado.to_string(),
        });
    }
    Ok(())
}

/// Retorna o limite padrão de segurança para desserialização com Serde.
fn default_max_bytes() -> u64 {
    LIMITE_PADRAO_BYTES
}

/// Retorna `true` para opções de segurança habilitadas por padrão.
fn default_true() -> bool {
    true
}

/// Normaliza uma fonte Base64 removendo espaços e um prefixo Data URI opcional.
struct LeitorBase64Normalizado<R> {
    origem: R,
    pendente: Vec<u8>,
    posicao: usize,
    buffer_origem: [u8; 64 * 1024],
    fim: bool,
    primeiro_bloco: bool,
    permitir_data_uri: bool,
    cabecalho_incompleto: Vec<u8>,
}

/// Inicializa o normalizador sem materializar o conteúdo completo na memória.
impl<R: Read> LeitorBase64Normalizado<R> {
    /// Cria um leitor normalizado para a fonte e política de Data URI informadas.
    fn new(origem: R, permitir_data_uri: bool) -> Self {
        Self {
            origem,
            pendente: Vec::with_capacity(64 * 1024),
            posicao: 0,
            buffer_origem: [0; 64 * 1024],
            fim: false,
            primeiro_bloco: true,
            permitir_data_uri,
            cabecalho_incompleto: Vec::new(),
        }
    }

    /// Reabastece o buffer já sem espaços e sem metadados Data URI.
    fn reabastecer(&mut self) -> io::Result<()> {
        self.pendente.clear();
        self.posicao = 0;
        while self.pendente.is_empty() && !self.fim {
            let quantidade = self.origem.read(&mut self.buffer_origem)?;
            if quantidade == 0 {
                self.fim = true;
                if self.primeiro_bloco && !self.cabecalho_incompleto.is_empty() {
                    self.processar_primeiro_conteudo(&[])?;
                }
                break;
            }
            if self.primeiro_bloco {
                let trecho = self.buffer_origem[..quantidade].to_vec();
                self.processar_primeiro_conteudo(&trecho)?;
            } else {
                self.pendente.extend(
                    self.buffer_origem[..quantidade]
                        .iter()
                        .copied()
                        .filter(|byte| !byte.is_ascii_whitespace()),
                );
            }
        }
        Ok(())
    }

    /// Decide se a entrada começa com Data URI e descarta somente seu cabeçalho.
    fn processar_primeiro_conteudo(&mut self, trecho: &[u8]) -> io::Result<()> {
        self.cabecalho_incompleto.extend_from_slice(trecho);

        // Espaços antes do primeiro caractere útil não pertencem nem ao
        // cabeçalho Data URI nem ao conteúdo Base64.
        let quantidade_espacos_iniciais = self
            .cabecalho_incompleto
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .unwrap_or(self.cabecalho_incompleto.len());
        if quantidade_espacos_iniciais > 0 {
            self.cabecalho_incompleto
                .drain(..quantidade_espacos_iniciais);
        }

        let conteudo = &self.cabecalho_incompleto;
        if conteudo.len() < 5 && !trecho.is_empty() {
            return Ok(());
        }
        let inicia_data = conteudo.len() >= 5 && conteudo[..5].eq_ignore_ascii_case(b"data:");

        if inicia_data {
            if !self.permitir_data_uri {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "prefixo Data URI não permitido",
                ));
            }
            let posicao_virgula = conteudo.iter().position(|byte| *byte == b',');
            let Some(posicao_virgula) = posicao_virgula else {
                if conteudo.len() > LIMITE_CABECALHO_DATA_URI {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "cabeçalho Data URI excede o limite permitido",
                    ));
                }
                return Ok(());
            };
            if posicao_virgula > LIMITE_CABECALHO_DATA_URI {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "cabeçalho Data URI excede o limite permitido",
                ));
            }
            let cabecalho = &conteudo[..posicao_virgula];
            if !cabecalho
                .windows(7)
                .any(|janela| janela.eq_ignore_ascii_case(b";base64"))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Data URI não declara codificação Base64",
                ));
            }
            self.pendente.extend(
                conteudo[posicao_virgula + 1..]
                    .iter()
                    .copied()
                    .filter(|byte| !byte.is_ascii_whitespace()),
            );
        } else {
            self.pendente.extend(
                conteudo
                    .iter()
                    .copied()
                    .filter(|byte| !byte.is_ascii_whitespace()),
            );
        }

        self.primeiro_bloco = false;
        self.cabecalho_incompleto.clear();
        Ok(())
    }
}

/// Entrega ao decodificador apenas caracteres Base64 normalizados.
impl<R: Read> Read for LeitorBase64Normalizado<R> {
    /// Copia o próximo trecho normalizado para o buffer solicitado.
    fn read(&mut self, buffer_saida: &mut [u8]) -> io::Result<usize> {
        if buffer_saida.is_empty() {
            return Ok(0);
        }
        if self.posicao >= self.pendente.len() {
            self.reabastecer()?;
        }
        if self.posicao >= self.pendente.len() {
            return Ok(0);
        }
        let disponivel = &self.pendente[self.posicao..];
        let quantidade = disponivel.len().min(buffer_saida.len());
        buffer_saida[..quantidade].copy_from_slice(&disponivel[..quantidade]);
        self.posicao += quantidade;
        Ok(quantidade)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Confirma que Data URI e espaços são removidos em streaming.
    #[test]
    fn normaliza_data_uri_e_espacos() {
        let entrada = Cursor::new(b" data:text/plain;base64,\nSGVs bG8=".to_vec());
        let mut leitor = LeitorBase64Normalizado::new(entrada, true);
        let mut resultado = Vec::new();
        leitor.read_to_end(&mut resultado).unwrap();
        assert_eq!(resultado, b"SGVsbG8=");
    }

    /// Impede regressão em Base64 contínuo maior que o limite de cabeçalho.
    #[test]
    fn normaliza_base64_continuo_grande_sem_confundir_com_data_uri() {
        let entrada_original = vec![0xA5_u8; 32 * 1024];
        let entrada_base64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &entrada_original,
        );
        let mut leitor = LeitorBase64Normalizado::new(Cursor::new(entrada_base64.as_bytes()), true);
        let mut resultado = Vec::new();
        leitor.read_to_end(&mut resultado).unwrap();
        assert_eq!(resultado, entrada_base64.as_bytes());
    }

    /// Confirma que o limite alcança o cabeçalho, não o payload Data URI.
    #[test]
    fn aceita_data_uri_com_payload_maior_que_o_limite_de_cabecalho() {
        let payload = "QQ==".repeat(4 * 1024);
        let entrada = format!("data:application/octet-stream;base64,{payload}");
        let mut leitor = LeitorBase64Normalizado::new(Cursor::new(entrada.as_bytes()), true);
        let mut resultado = Vec::new();
        leitor.read_to_end(&mut resultado).unwrap();
        assert_eq!(resultado, payload.as_bytes());
    }

    /// Confirma que um arquivo repetido com mesmo hash é idempotente.
    #[test]
    fn reconhece_saida_idempotente() {
        let diretorio = tempfile::tempdir().unwrap();
        let caminho = diretorio.path().join("arquivo.txt");
        fs::write(&caminho, b"Hello").unwrap();
        let hash_calculado = sha256_file(&caminho).unwrap();
        assert!(arquivo_existente_identico(&caminho, 5, &hash_calculado).unwrap());
    }
}
