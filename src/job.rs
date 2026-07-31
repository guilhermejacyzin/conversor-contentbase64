use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::decoder::{
    Base64Variant, DecodeOptions, DecodeResult, decode_base64_reader_to_file, decode_base64_to_file,
};
use crate::error::{ConverterError, Result};
use crate::expansion::{ExpansionPolicy, ExpansionResult, expand_containers};
use crate::path_policy::{canonical_source_within, sanitize_filename, validate_relative_path};

const VERSAO_CONTRATO: &str = "base64-file-converter.job.v1";

/// Envelope genérico de um trabalho recebido de API, fila, scheduler ou pasta.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentJob {
    /// Versão obrigatória do contrato de entrada.
    #[serde(rename = "schema_version")]
    pub versao_esquema: String,
    /// Identificador único e idempotente do trabalho.
    #[serde(rename = "job_id")]
    pub identificador_trabalho: String,
    /// Identificador compartilhado com a entidade de origem e a persistência.
    #[serde(rename = "correlation_id")]
    pub identificador_correlacao: String,
    /// Sistema que produziu o trabalho, sem criar dependência funcional.
    #[serde(rename = "source_system")]
    pub sistema_origem: String,
    /// Categoria livre do fluxo de negócio definida pelo sistema integrador.
    #[serde(rename = "flow", default)]
    pub fluxo: Option<String>,
    /// Identificador da operação no contrato do sistema de origem.
    #[serde(rename = "operation_id", default)]
    pub identificador_operacao: Option<String>,
    /// Tag técnica opcional definida pela integração.
    #[serde(rename = "integration_tag", default)]
    pub etiqueta_integracao: Option<String>,
    /// Chaves de negócio sem semântica fixa no núcleo do conversor.
    #[serde(rename = "business_keys", default)]
    pub chaves_negocio: BTreeMap<String, String>,
    /// Metadados adicionais transportados sem interpretação.
    #[serde(rename = "metadata", default)]
    pub metadados: BTreeMap<String, Value>,
    /// Política técnica aplicada a todos os anexos do trabalho.
    #[serde(rename = "policy", default)]
    pub politica: JobPolicy,
    /// Anexos que devem ser reconstruídos e opcionalmente expandidos.
    #[serde(rename = "attachments")]
    pub anexos: Vec<AttachmentRequest>,
}

/// Define o comportamento técnico do trabalho sem conter regras de negócio.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobPolicy {
    /// Limite de bytes decodificados por anexo.
    #[serde(
        rename = "max_decoded_bytes_per_attachment",
        default = "limite_decodificado_padrao"
    )]
    pub maximo_decodificado_por_anexo: u64,
    /// Preserva extensões desconhecidas em vez de descartar o anexo.
    #[serde(rename = "preserve_unknown_formats", default = "padrao_verdadeiro")]
    pub preservar_formatos_desconhecidos: bool,
    /// Continua processando anexos independentes após uma falha.
    #[serde(
        rename = "continue_after_attachment_error",
        default = "padrao_verdadeiro"
    )]
    pub continuar_apos_erro_anexo: bool,
    /// Classifica o trabalho como quarentena quando qualquer anexo falha.
    #[serde(
        rename = "quarantine_on_attachment_error",
        default = "padrao_verdadeiro"
    )]
    pub quarentena_em_erro_anexo: bool,
    /// Sincroniza cada evento de auditoria antes de retornar ao processamento.
    #[serde(rename = "fsync_audit_events", default)]
    pub sincronizar_eventos_auditoria: bool,
    /// Política completa de expansão recursiva.
    #[serde(rename = "expansion", default)]
    pub expansao: ExpansionPolicy,
}

/// Fornece uma política segura e capaz de continuar anexos independentes.
impl Default for JobPolicy {
    /// Cria a política padrão de reconstrução, auditoria e tratamento de falhas.
    fn default() -> Self {
        Self {
            maximo_decodificado_por_anexo: limite_decodificado_padrao(),
            preservar_formatos_desconhecidos: true,
            continuar_apos_erro_anexo: true,
            quarentena_em_erro_anexo: true,
            sincronizar_eventos_auditoria: false,
            expansao: ExpansionPolicy::default(),
        }
    }
}

/// Descreve uma única fonte Base64 e seu destino relativo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRequest {
    /// Identificador único do anexo dentro do trabalho.
    #[serde(rename = "attachment_id")]
    pub identificador_anexo: String,
    /// Nome original exatamente recebido do sistema de origem.
    #[serde(rename = "filename")]
    pub nome_arquivo: String,
    /// Caminho relativo para um arquivo que contém somente o Base64.
    #[serde(rename = "base64_path", default)]
    pub caminho_base64: Option<PathBuf>,
    /// Conteúdo Base64 embutido; permitido para integração simples, não recomendado em escala.
    #[serde(rename = "content_base64", default)]
    pub conteudo_base64: Option<String>,
    /// Caminho de saída relativo à raiz fornecida em tempo de execução.
    #[serde(rename = "output_relative_path")]
    pub caminho_relativo_saida: PathBuf,
    /// Tamanho esperado do arquivo original, quando a origem o fornece.
    #[serde(rename = "expected_size_bytes", default)]
    pub tamanho_esperado_bytes: Option<u64>,
    /// SHA-256 esperado do arquivo original, quando a origem o fornece.
    #[serde(rename = "expected_sha256", default)]
    pub sha256_esperado: Option<String>,
    /// Tipo MIME declarado pela origem, quando disponível.
    #[serde(rename = "expected_media_type", default)]
    pub tipo_mime_esperado: Option<String>,
    /// Variante Base64 utilizada pela origem.
    #[serde(rename = "base64_variant", default)]
    pub variante_base64: Base64Variant,
    /// Permite desabilitar expansão para um anexo específico.
    #[serde(rename = "expand_containers", default = "padrao_verdadeiro")]
    pub expandir_containers: bool,
    /// Metadados exclusivos do anexo preservados no resultado.
    #[serde(rename = "metadata", default)]
    pub metadados: BTreeMap<String, Value>,
}

/// Classifica o resultado final de um trabalho.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    /// Todos os anexos foram reconstruídos e expandidos conforme esperado.
    Completed,
    /// Arquivos foram preservados, mas existem avisos ou expansão incompleta.
    Partial,
    /// Pelo menos um anexo foi isolado para investigação e reprocessamento.
    Quarantined,
    /// O trabalho não produziu nenhum anexo válido.
    Failed,
}

/// Classifica o resultado individual de um anexo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentStatus {
    /// Reconstrução e expansão concluídas.
    Completed,
    /// Arquivo reconstruído, porém com avisos ou expansão incompleta.
    Partial,
    /// Fonte preservada, mas reconstrução ou validação falhou.
    Quarantined,
    /// Processamento interrompido por política de falha imediata.
    Failed,
}

/// Resultado auditável de um anexo, sem repetir seu conteúdo Base64.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentResult {
    /// Identificador recebido no contrato de entrada.
    #[serde(rename = "attachment_id")]
    pub identificador_anexo: String,
    /// Nome original recebido.
    #[serde(rename = "filename")]
    pub nome_arquivo: String,
    /// Estado final do anexo.
    #[serde(rename = "status")]
    pub estado: AttachmentStatus,
    /// Metadados transportados sem alteração.
    #[serde(rename = "metadata")]
    pub metadados: BTreeMap<String, Value>,
    /// Resultado de reconstrução, quando a publicação ocorreu.
    #[serde(rename = "decode", skip_serializing_if = "Option::is_none")]
    pub decodificacao: Option<DecodeResult>,
    /// Resultado de expansão recursiva, quando solicitado.
    #[serde(rename = "expansion", skip_serializing_if = "Option::is_none")]
    pub expansao: Option<ExpansionResult>,
    /// Erro sanitizado, sem conteúdo do arquivo ou credenciais.
    #[serde(rename = "error", skip_serializing_if = "Option::is_none")]
    pub erro: Option<String>,
}

/// Consolida o estado, métricas e linhagem de um trabalho completo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResult {
    /// Versão do contrato de resultado.
    #[serde(rename = "schema_version")]
    pub versao_esquema: String,
    /// Identificador idempotente do trabalho.
    #[serde(rename = "job_id")]
    pub identificador_trabalho: String,
    /// Identificador compartilhado com as demais camadas da solução.
    #[serde(rename = "correlation_id")]
    pub identificador_correlacao: String,
    /// Sistema que originou o trabalho.
    #[serde(rename = "source_system")]
    pub sistema_origem: String,
    /// Fluxo livre recebido.
    #[serde(rename = "flow", skip_serializing_if = "Option::is_none")]
    pub fluxo: Option<String>,
    /// Identificador da operação de origem.
    #[serde(rename = "operation_id", skip_serializing_if = "Option::is_none")]
    pub identificador_operacao: Option<String>,
    /// Tag de integração recebida.
    #[serde(rename = "integration_tag", skip_serializing_if = "Option::is_none")]
    pub etiqueta_integracao: Option<String>,
    /// Chaves para relacionamento com banco e API.
    #[serde(rename = "business_keys")]
    pub chaves_negocio: BTreeMap<String, String>,
    /// Metadados gerais transportados.
    #[serde(rename = "metadata")]
    pub metadados: BTreeMap<String, Value>,
    /// Estado final calculado.
    #[serde(rename = "status")]
    pub estado: JobStatus,
    /// Instante UTC de início.
    #[serde(rename = "started_at")]
    pub iniciado_em: String,
    /// Instante UTC de término.
    #[serde(rename = "finished_at")]
    pub finalizado_em: String,
    /// Duração total do trabalho.
    #[serde(rename = "duration_ms")]
    pub duracao_ms: u128,
    /// Soma dos bytes originais reconstruídos.
    #[serde(rename = "decoded_bytes")]
    pub bytes_decodificados: u64,
    /// Soma dos filhos extraídos.
    #[serde(rename = "extracted_bytes")]
    pub bytes_extraidos: u64,
    /// Resultados individuais, na mesma ordem do manifesto.
    #[serde(rename = "attachments")]
    pub anexos: Vec<AttachmentResult>,
}

/// Evento append-only persistido no arquivo de auditoria do trabalho.
#[derive(Debug, Serialize)]
struct EventoAuditoria<'a> {
    #[serde(rename = "sequence")]
    sequencia: u64,
    #[serde(rename = "timestamp")]
    instante: String,
    #[serde(rename = "job_id")]
    identificador_trabalho: &'a str,
    #[serde(rename = "correlation_id")]
    identificador_correlacao: &'a str,
    #[serde(rename = "event")]
    evento: &'a str,
    #[serde(rename = "status")]
    estado: &'a str,
    #[serde(rename = "attachment_id", skip_serializing_if = "Option::is_none")]
    identificador_anexo: Option<&'a str>,
    #[serde(rename = "details", skip_serializing_if = "Option::is_none")]
    detalhes: Option<Value>,
}

/// Escritor compartilhado que serializa eventos JSONL sem intercalar linhas.
struct GravadorAuditoria {
    arquivo: Mutex<File>,
    sequencia: AtomicU64,
    sincronizar: bool,
    identificador_trabalho: String,
    identificador_correlacao: String,
}

/// Implementa persistência durável de eventos de auditoria.
impl GravadorAuditoria {
    /// Abre o log append-only do trabalho e preserva eventos de tentativas anteriores.
    fn abrir(caminho: &Path, trabalho: &AttachmentJob, sincronizar: bool) -> Result<Self> {
        let arquivo = OpenOptions::new()
            .create(true)
            .append(true)
            .open(caminho)
            .map_err(|origem| ConverterError::io(caminho, origem))?;
        Ok(Self {
            arquivo: Mutex::new(arquivo),
            sequencia: AtomicU64::new(0),
            sincronizar,
            identificador_trabalho: trabalho.identificador_trabalho.clone(),
            identificador_correlacao: trabalho.identificador_correlacao.clone(),
        })
    }

    /// Acrescenta um evento completo, faz `flush` e opcionalmente `sync_data`.
    fn registrar(
        &self,
        evento: &str,
        estado: &str,
        identificador_anexo: Option<&str>,
        detalhes: Option<Value>,
    ) -> Result<()> {
        let registro = EventoAuditoria {
            sequencia: self.sequencia.fetch_add(1, Ordering::SeqCst) + 1,
            instante: agora_rfc3339(),
            identificador_trabalho: &self.identificador_trabalho,
            identificador_correlacao: &self.identificador_correlacao,
            evento,
            estado,
            identificador_anexo,
            detalhes,
        };
        let linha = serde_json::to_vec(&registro)?;
        let mut arquivo = self
            .arquivo
            .lock()
            .map_err(|_| ConverterError::Spool("mutex de auditoria foi invalidado".to_string()))?;
        arquivo
            .write_all(&linha)
            .and_then(|_| arquivo.write_all(b"\n"))
            .and_then(|_| arquivo.flush())
            .map_err(|origem| ConverterError::io("audit.jsonl", origem))?;
        if self.sincronizar {
            arquivo
                .sync_data()
                .map_err(|origem| ConverterError::io("audit.jsonl", origem))?;
        }
        Ok(())
    }
}

/// Lê, valida e processa um diretório de trabalho independente do gatilho.
///
/// O diretório deve conter `job.json` e as fontes referenciadas por caminhos
/// relativos. O resultado não é gravado por esta função; o chamador escolhe API,
/// banco, fila ou spool como destino.
pub fn process_job_directory(diretorio_trabalho: &Path, raiz_saida: &Path) -> Result<JobResult> {
    let inicio = Instant::now();
    let caminho_manifesto = diretorio_trabalho.join("job.json");
    let arquivo_manifesto = File::open(&caminho_manifesto)
        .map_err(|origem| ConverterError::io(&caminho_manifesto, origem))?;
    let trabalho: AttachmentJob = serde_json::from_reader(BufReader::new(arquivo_manifesto))?;
    validar_trabalho(&trabalho)?;

    fs::create_dir_all(raiz_saida).map_err(|origem| ConverterError::io(raiz_saida, origem))?;
    let raiz_saida_canonica =
        fs::canonicalize(raiz_saida).map_err(|origem| ConverterError::io(raiz_saida, origem))?;
    let auditoria = Arc::new(GravadorAuditoria::abrir(
        &diretorio_trabalho.join("audit.jsonl"),
        &trabalho,
        trabalho.politica.sincronizar_eventos_auditoria,
    )?);
    auditoria.registrar(
        "job_started",
        "processing",
        None,
        Some(serde_json::json!({
            "attachment_count": trabalho.anexos.len(),
            "flow": trabalho.fluxo,
            "operation_id": trabalho.identificador_operacao,
        })),
    )?;

    let iniciado_em = agora_rfc3339();
    let processar = |anexo: &AttachmentRequest| {
        processar_anexo(
            diretorio_trabalho,
            &raiz_saida_canonica,
            &trabalho,
            anexo,
            &auditoria,
        )
    };
    let resultados = if trabalho.politica.continuar_apos_erro_anexo {
        trabalho
            .anexos
            .par_iter()
            .map(processar)
            .collect::<Vec<_>>()
    } else {
        processar_sequencial_ate_falha(&trabalho.anexos, processar)
    };

    let estado = calcular_estado_trabalho(&resultados, &trabalho.politica);
    let bytes_decodificados = resultados
        .iter()
        .filter_map(|resultado| resultado.decodificacao.as_ref())
        .map(|resultado| resultado.tamanho_decodificado_bytes)
        .sum();
    let bytes_extraidos = resultados
        .iter()
        .filter_map(|resultado| resultado.expansao.as_ref())
        .map(|resultado| resultado.total_extraido_bytes)
        .sum();
    let resultado = JobResult {
        versao_esquema: "base64-file-converter.result.v1".to_string(),
        identificador_trabalho: trabalho.identificador_trabalho.clone(),
        identificador_correlacao: trabalho.identificador_correlacao.clone(),
        sistema_origem: trabalho.sistema_origem.clone(),
        fluxo: trabalho.fluxo.clone(),
        identificador_operacao: trabalho.identificador_operacao.clone(),
        etiqueta_integracao: trabalho.etiqueta_integracao.clone(),
        chaves_negocio: trabalho.chaves_negocio.clone(),
        metadados: trabalho.metadados.clone(),
        estado,
        iniciado_em,
        finalizado_em: agora_rfc3339(),
        duracao_ms: inicio.elapsed().as_millis(),
        bytes_decodificados,
        bytes_extraidos,
        anexos: resultados,
    };
    auditoria.registrar(
        "job_finished",
        nome_estado_trabalho(estado),
        None,
        Some(serde_json::json!({
            "duration_ms": resultado.duracao_ms,
            "decoded_bytes": resultado.bytes_decodificados,
            "extracted_bytes": resultado.bytes_extraidos,
        })),
    )?;
    Ok(resultado)
}

/// Processa anexos em ordem e marca os restantes como não executados após falha.
fn processar_sequencial_ate_falha<F>(
    anexos: &[AttachmentRequest],
    mut processar: F,
) -> Vec<AttachmentResult>
where
    F: FnMut(&AttachmentRequest) -> AttachmentResult,
{
    let mut resultados = Vec::with_capacity(anexos.len());
    let mut interrompido = false;
    for anexo in anexos {
        if interrompido {
            resultados.push(AttachmentResult {
                identificador_anexo: anexo.identificador_anexo.clone(),
                nome_arquivo: anexo.nome_arquivo.clone(),
                estado: AttachmentStatus::Failed,
                metadados: anexo.metadados.clone(),
                decodificacao: None,
                expansao: None,
                erro: Some("não processado após falha anterior por política do job".to_string()),
            });
            continue;
        }
        let resultado = processar(anexo);
        interrompido = matches!(
            resultado.estado,
            AttachmentStatus::Quarantined | AttachmentStatus::Failed
        );
        resultados.push(resultado);
    }
    resultados
}

/// Reconstrói, valida, expande e audita um único anexo.
fn processar_anexo(
    diretorio_trabalho: &Path,
    raiz_saida: &Path,
    trabalho: &AttachmentJob,
    anexo: &AttachmentRequest,
    auditoria: &GravadorAuditoria,
) -> AttachmentResult {
    let _ = auditoria.registrar(
        "attachment_started",
        "processing",
        Some(&anexo.identificador_anexo),
        Some(serde_json::json!({
            "output_relative_path": caminho_portavel(&anexo.caminho_relativo_saida),
        })),
    );

    let resultado = (|| -> Result<(DecodeResult, ExpansionResult)> {
        let relativo = validate_relative_path(&anexo.caminho_relativo_saida)?;
        let caminho_saida = raiz_saida.join(&relativo);
        garantir_destino_na_raiz(raiz_saida, &caminho_saida)?;
        let nome_armazenado = caminho_saida
            .file_name()
            .and_then(|valor| valor.to_str())
            .ok_or_else(|| ConverterError::UnsafeFilename(caminho_saida.display().to_string()))?;
        let nome_logico = sanitize_filename(&anexo.nome_arquivo, nome_armazenado);
        let opcoes = DecodeOptions {
            maximo_bytes_decodificados: trabalho.politica.maximo_decodificado_por_anexo,
            tamanho_esperado_bytes: anexo.tamanho_esperado_bytes,
            sha256_esperado: anexo.sha256_esperado.clone(),
            tipo_mime_esperado: anexo.tipo_mime_esperado.clone(),
            validar_assinatura: true,
            preservar_formatos_desconhecidos: trabalho.politica.preservar_formatos_desconhecidos,
            sincronizar_antes_publicacao: true,
            variante_base64: anexo.variante_base64,
            permitir_prefixo_data_uri: true,
        };
        let decodificacao = if let Some(caminho_base64) = &anexo.caminho_base64 {
            let fonte = canonical_source_within(diretorio_trabalho, caminho_base64)?;
            decode_base64_to_file(&fonte, &caminho_saida, &nome_logico, &opcoes)?
        } else if let Some(conteudo) = &anexo.conteudo_base64 {
            decode_base64_reader_to_file(
                Cursor::new(conteudo.as_bytes()),
                &caminho_saida,
                &nome_logico,
                &opcoes,
            )?
        } else {
            return Err(ConverterError::InvalidJob(format!(
                "anexo {} não possui fonte Base64",
                anexo.identificador_anexo
            )));
        };

        let mut politica_expansao = trabalho.politica.expansao.clone();
        politica_expansao.habilitada &= anexo.expandir_containers;
        let expansao = expand_containers(
            &caminho_saida,
            raiz_saida,
            &nome_logico,
            &anexo.identificador_anexo,
            &politica_expansao,
        )?;
        Ok((decodificacao, expansao))
    })();

    match resultado {
        Ok((decodificacao, expansao)) => {
            let estado = if expansao.completa {
                AttachmentStatus::Completed
            } else {
                AttachmentStatus::Partial
            };
            let _ = auditoria.registrar(
                "attachment_finished",
                nome_estado_anexo(estado),
                Some(&anexo.identificador_anexo),
                Some(serde_json::json!({
                    "sha256": decodificacao.sha256,
                    "decoded_bytes": decodificacao.tamanho_decodificado_bytes,
                    "extracted_files": expansao.quantidade_arquivos_extraidos,
                    "extracted_bytes": expansao.total_extraido_bytes,
                })),
            );
            AttachmentResult {
                identificador_anexo: anexo.identificador_anexo.clone(),
                nome_arquivo: anexo.nome_arquivo.clone(),
                estado,
                metadados: anexo.metadados.clone(),
                decodificacao: Some(decodificacao),
                expansao: Some(expansao),
                erro: None,
            }
        }
        Err(erro) => {
            let mensagem = sanitizar_erro(&erro.to_string());
            let _ = auditoria.registrar(
                "attachment_failed",
                "quarantined",
                Some(&anexo.identificador_anexo),
                Some(serde_json::json!({ "error": mensagem })),
            );
            AttachmentResult {
                identificador_anexo: anexo.identificador_anexo.clone(),
                nome_arquivo: anexo.nome_arquivo.clone(),
                estado: AttachmentStatus::Quarantined,
                metadados: anexo.metadados.clone(),
                decodificacao: None,
                expansao: None,
                erro: Some(mensagem),
            }
        }
    }
}

/// Valida versão, identificadores, fontes e destinos antes de qualquer decodificação.
fn validar_trabalho(trabalho: &AttachmentJob) -> Result<()> {
    if trabalho.versao_esquema != VERSAO_CONTRATO {
        return Err(ConverterError::InvalidJob(format!(
            "schema_version deve ser {VERSAO_CONTRATO}"
        )));
    }
    validar_identificador("job_id", &trabalho.identificador_trabalho)?;
    validar_identificador("correlation_id", &trabalho.identificador_correlacao)?;
    if trabalho.sistema_origem.trim().is_empty() {
        return Err(ConverterError::InvalidJob(
            "source_system é obrigatório".to_string(),
        ));
    }
    if trabalho.anexos.is_empty() {
        return Err(ConverterError::InvalidJob(
            "o trabalho deve conter pelo menos um anexo".to_string(),
        ));
    }

    let mut identificadores = HashSet::new();
    let mut destinos = HashSet::new();
    for anexo in &trabalho.anexos {
        validar_identificador("attachment_id", &anexo.identificador_anexo)?;
        if !identificadores.insert(anexo.identificador_anexo.to_ascii_lowercase()) {
            return Err(ConverterError::InvalidJob(format!(
                "attachment_id duplicado: {}",
                anexo.identificador_anexo
            )));
        }
        if anexo.nome_arquivo.trim().is_empty() {
            return Err(ConverterError::InvalidJob(format!(
                "filename vazio no anexo {}",
                anexo.identificador_anexo
            )));
        }
        let quantidade_fontes = usize::from(anexo.caminho_base64.is_some())
            + usize::from(anexo.conteudo_base64.is_some());
        if quantidade_fontes != 1 {
            return Err(ConverterError::InvalidJob(format!(
                "anexo {} deve informar exatamente uma fonte: base64_path ou content_base64",
                anexo.identificador_anexo
            )));
        }
        let relativo = validate_relative_path(&anexo.caminho_relativo_saida)?;
        let chave_destino = caminho_portavel(&relativo).to_ascii_lowercase();
        if !destinos.insert(chave_destino) {
            return Err(ConverterError::InvalidJob(format!(
                "output_relative_path duplicado: {}",
                anexo.caminho_relativo_saida.display()
            )));
        }
    }
    Ok(())
}

/// Valida identificadores usados em caminhos, logs e correlação.
fn validar_identificador(campo: &str, valor: &str) -> Result<()> {
    let tamanho = valor.chars().count();
    if tamanho == 0
        || tamanho > 128
        || !valor
            .chars()
            .all(|caractere| caractere.is_ascii_alphanumeric() || "-_.:@".contains(caractere))
    {
        return Err(ConverterError::InvalidJob(format!(
            "{campo} deve conter de 1 a 128 caracteres alfanuméricos ou -_.:@"
        )));
    }
    Ok(())
}

/// Garante que o destino normalizado permanece sob a raiz configurada.
fn garantir_destino_na_raiz(raiz: &Path, destino: &Path) -> Result<()> {
    let diretorio_pai = destino
        .parent()
        .ok_or_else(|| ConverterError::UnsafeRelativePath(destino.display().to_string()))?;
    fs::create_dir_all(diretorio_pai)
        .map_err(|origem| ConverterError::io(diretorio_pai, origem))?;
    let pai_canonico = fs::canonicalize(diretorio_pai)
        .map_err(|origem| ConverterError::io(diretorio_pai, origem))?;
    if !pai_canonico.starts_with(raiz) {
        return Err(ConverterError::SourcePathEscape(
            destino.display().to_string(),
        ));
    }
    Ok(())
}

/// Calcula o estado do trabalho a partir de todos os anexos.
fn calcular_estado_trabalho(resultados: &[AttachmentResult], politica: &JobPolicy) -> JobStatus {
    let concluidos = resultados
        .iter()
        .filter(|resultado| resultado.estado == AttachmentStatus::Completed)
        .count();
    let parciais = resultados
        .iter()
        .any(|resultado| resultado.estado == AttachmentStatus::Partial);
    let falhas = resultados.iter().any(|resultado| {
        matches!(
            resultado.estado,
            AttachmentStatus::Quarantined | AttachmentStatus::Failed
        )
    });

    if falhas && concluidos == 0 && !parciais {
        JobStatus::Failed
    } else if falhas && politica.quarentena_em_erro_anexo {
        JobStatus::Quarantined
    } else if falhas || parciais {
        JobStatus::Partial
    } else {
        JobStatus::Completed
    }
}

/// Retorna o nome externo estável de um estado de trabalho.
fn nome_estado_trabalho(estado: JobStatus) -> &'static str {
    match estado {
        JobStatus::Completed => "completed",
        JobStatus::Partial => "partial",
        JobStatus::Quarantined => "quarantined",
        JobStatus::Failed => "failed",
    }
}

/// Retorna o nome externo estável de um estado de anexo.
fn nome_estado_anexo(estado: AttachmentStatus) -> &'static str {
    match estado {
        AttachmentStatus::Completed => "completed",
        AttachmentStatus::Partial => "partial",
        AttachmentStatus::Quarantined => "quarantined",
        AttachmentStatus::Failed => "failed",
    }
}

/// Remove quebras de linha e limita mensagens antes de gravá-las em JSONL.
fn sanitizar_erro(mensagem: &str) -> String {
    mensagem
        .replace(['\r', '\n'], " ")
        .chars()
        .take(2_000)
        .collect()
}

/// Produz um timestamp UTC em RFC 3339 para interoperabilidade.
fn agora_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Converte um caminho relativo para o formato portátil do JSON.
fn caminho_portavel(caminho: &Path) -> String {
    caminho
        .components()
        .map(|componente| componente.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Retorna o limite padrão de 512 MiB decodificados por anexo.
fn limite_decodificado_padrao() -> u64 {
    512 * 1024 * 1024
}

/// Retorna `true` para políticas de preservação habilitadas por padrão.
fn padrao_verdadeiro() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirma que identificadores aceitam UUIDs e rejeitam travessia.
    #[test]
    fn valida_identificadores_operacionais() {
        assert!(validar_identificador("job_id", "job-1234_abcd").is_ok());
        assert!(validar_identificador("job_id", "../segredo").is_err());
    }

    /// Confirma que estado parcial não é apresentado como sucesso integral.
    #[test]
    fn calcula_estado_parcial() {
        let resultado = AttachmentResult {
            identificador_anexo: "a1".to_string(),
            nome_arquivo: "arquivo.zip".to_string(),
            estado: AttachmentStatus::Partial,
            metadados: BTreeMap::new(),
            decodificacao: None,
            expansao: None,
            erro: None,
        };
        assert_eq!(
            calcular_estado_trabalho(&[resultado], &JobPolicy::default()),
            JobStatus::Partial
        );
    }
}
