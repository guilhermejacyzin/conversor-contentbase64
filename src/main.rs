//! Interface de linha de comando do conversor portátil.
//!
//! Expõe reconstrução direta, processamento de manifestos, worker de spool e
//! declaração de capacidades sem incorporar regras de negócio.

use std::io::{self};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use base64_file_converter::{
    Base64Variant, DecodeOptions, SpoolOptions, decode_base64_reader_to_file,
    decode_base64_to_file, process_job_directory, run_spool, write_json_atomic,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

/// Interface portátil para reconstrução, jobs e execução contínua.
#[derive(Debug, Parser)]
#[command(
    name = "base64-file-converter",
    version,
    about = "Reconstrói anexos Base64 com integridade, auditoria e expansão recursiva segura"
)]
struct LinhaComando {
    /// Operação solicitada pelo adaptador externo.
    #[command(subcommand)]
    comando: Comando,
}

/// Operações disponíveis sem acoplamento a um gatilho específico.
#[derive(Debug, Subcommand)]
enum Comando {
    /// Decodifica uma única fonte Base64 em streaming.
    Decode(ComandoDecodificar),
    /// Processa um diretório contendo `job.json`.
    ProcessJob(ComandoProcessarJob),
    /// Processa uma fila de diretórios uma vez ou continuamente.
    Spool(ComandoSpool),
    /// Exibe capacidades e fronteiras técnicas em JSON.
    Capabilities,
}

/// Argumentos da reconstrução direta de um arquivo.
#[derive(Debug, Args)]
struct ComandoDecodificar {
    /// Arquivo Base64 de entrada; use `-` para stdin.
    #[arg(long)]
    entrada: String,
    /// Arquivo original a ser publicado.
    #[arg(long)]
    saida: PathBuf,
    /// Nome lógico usado para validar extensão e assinatura.
    #[arg(long)]
    nome_arquivo: String,
    /// Limite de bytes após a decodificação.
    #[arg(long, default_value_t = 512 * 1024 * 1024)]
    maximo_bytes: u64,
    /// Tamanho esperado do arquivo original.
    #[arg(long)]
    tamanho_esperado: Option<u64>,
    /// SHA-256 esperado do arquivo original.
    #[arg(long)]
    sha256_esperado: Option<String>,
    /// MIME esperado do arquivo original.
    #[arg(long)]
    tipo_mime_esperado: Option<String>,
    /// Alfabeto Base64 recebido.
    #[arg(long, value_enum, default_value_t = VarianteCli::Standard)]
    variante_base64: VarianteCli,
    /// Desabilita validação de extensão e assinatura.
    #[arg(long, default_value_t = false)]
    sem_validacao_assinatura: bool,
    /// Bloqueia extensões fora do catálogo em vez de preservá-las.
    #[arg(long, default_value_t = false)]
    rejeitar_formatos_desconhecidos: bool,
}

/// Argumentos para um manifesto processado sob demanda.
#[derive(Debug, Args)]
struct ComandoProcessarJob {
    /// Diretório que contém `job.json` e fontes relativas.
    #[arg(long)]
    diretorio_job: PathBuf,
    /// Raiz de saída definida em tempo de execução.
    #[arg(long)]
    raiz_saida: PathBuf,
    /// Arquivo opcional para persistir o resultado; sem ele, usa stdout.
    #[arg(long)]
    resultado: Option<PathBuf>,
}

/// Argumentos do adaptador de spool.
#[derive(Debug, Args)]
struct ComandoSpool {
    /// Raiz que conterá inbox, processing, completed, quarantine e failed.
    #[arg(long)]
    raiz_spool: PathBuf,
    /// Raiz de publicação dos arquivos reconstruídos.
    #[arg(long)]
    raiz_saida: PathBuf,
    /// Mantém o processo ativo aguardando novos jobs.
    #[arg(long, default_value_t = false)]
    observar: bool,
    /// Intervalo entre varreduras no modo contínuo.
    #[arg(long, default_value_t = 1_000)]
    intervalo_ms: u64,
    /// Quantidade máxima de jobs simultâneos.
    #[arg(long)]
    concorrencia: Option<usize>,
    /// Quantidade máxima de jobs reivindicados por varredura.
    #[arg(long)]
    maximo_por_execucao: Option<usize>,
    /// Aceita job sem arquivo `READY`.
    #[arg(long, default_value_t = false)]
    sem_marcador_pronto: bool,
    /// Reprocessa itens existentes em `processing`.
    #[arg(long, default_value_t = false)]
    recuperar_processamento: bool,
}

/// Valores de linha de comando aceitos para o alfabeto Base64.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum VarianteCli {
    /// Base64 convencional.
    Standard,
    /// Base64 URL-safe.
    UrlSafe,
}

/// Converte a opção da CLI para o contrato interno serializável.
impl From<VarianteCli> for Base64Variant {
    /// Traduz a variante informada na linha de comando para a variante do núcleo.
    fn from(valor: VarianteCli) -> Self {
        match valor {
            VarianteCli::Standard => Base64Variant::Standard,
            VarianteCli::UrlSafe => Base64Variant::UrlSafe,
        }
    }
}

/// Documento JSON que explica capacidades sem depender da documentação externa.
#[derive(Debug, Serialize)]
struct Capacidades {
    #[serde(rename = "reconstruction")]
    reconstrucao: &'static str,
    #[serde(rename = "streaming")]
    usa_streaming: bool,
    #[serde(rename = "atomic_write")]
    escrita_atomica: bool,
    #[serde(rename = "integrity")]
    integridade: [&'static str; 2],
    #[serde(rename = "recursive_containers")]
    containers_recursivos: [&'static str; 8],
    #[serde(rename = "preserved_without_recursive_extractor")]
    preservados_sem_extrator_recursivo: [&'static str; 5],
    #[serde(rename = "office_documents_preserved_as_documents")]
    documentos_office_preservados: bool,
    #[serde(rename = "unknown_formats")]
    formatos_desconhecidos: &'static str,
    #[serde(rename = "triggers")]
    acionadores: [&'static str; 4],
    #[serde(rename = "security_limits")]
    limites_seguranca: [&'static str; 5],
}

/// Ponto de entrada que converte erros em código de saída não zero.
fn main() -> ExitCode {
    let argumentos = LinhaComando::parse();
    match executar(argumentos) {
        Ok(()) => ExitCode::SUCCESS,
        Err(erro) => {
            eprintln!("{erro}");
            ExitCode::FAILURE
        }
    }
}

/// Despacha o comando solicitado mantendo o núcleo independente da interface.
fn executar(argumentos: LinhaComando) -> base64_file_converter::Result<()> {
    match argumentos.comando {
        Comando::Decode(comando) => executar_decodificacao(comando),
        Comando::ProcessJob(comando) => executar_job(comando),
        Comando::Spool(comando) => executar_spool(comando),
        Comando::Capabilities => imprimir_json(&capacidades()),
    }
}

/// Executa reconstrução direta a partir de arquivo ou stdin.
fn executar_decodificacao(comando: ComandoDecodificar) -> base64_file_converter::Result<()> {
    let opcoes = DecodeOptions {
        maximo_bytes_decodificados: comando.maximo_bytes,
        tamanho_esperado_bytes: comando.tamanho_esperado,
        sha256_esperado: comando.sha256_esperado,
        tipo_mime_esperado: comando.tipo_mime_esperado,
        validar_assinatura: !comando.sem_validacao_assinatura,
        preservar_formatos_desconhecidos: !comando.rejeitar_formatos_desconhecidos,
        sincronizar_antes_publicacao: true,
        variante_base64: comando.variante_base64.into(),
        permitir_prefixo_data_uri: true,
    };
    let resultado = if comando.entrada == "-" {
        let entrada = io::stdin();
        decode_base64_reader_to_file(
            entrada.lock(),
            &comando.saida,
            &comando.nome_arquivo,
            &opcoes,
        )?
    } else {
        decode_base64_to_file(
            &PathBuf::from(comando.entrada),
            &comando.saida,
            &comando.nome_arquivo,
            &opcoes,
        )?
    };
    imprimir_json(&resultado)
}

/// Processa um manifesto e publica seu resultado em arquivo ou stdout.
fn executar_job(comando: ComandoProcessarJob) -> base64_file_converter::Result<()> {
    let resultado = process_job_directory(&comando.diretorio_job, &comando.raiz_saida)?;
    if let Some(caminho) = comando.resultado {
        write_json_atomic(&caminho, &resultado)
    } else {
        imprimir_json(&resultado)
    }
}

/// Executa uma ou várias varreduras do spool até sinal de encerramento.
fn executar_spool(comando: ComandoSpool) -> base64_file_converter::Result<()> {
    if comando.observar && comando.intervalo_ms == 0 {
        return Err(base64_file_converter::ConverterError::Spool(
            "intervalo_ms deve ser maior que zero no modo contínuo".to_string(),
        ));
    }
    let mut opcoes = SpoolOptions::default();
    if let Some(concorrencia) = comando.concorrencia {
        opcoes.maximo_jobs_concorrentes = concorrencia;
    }
    if let Some(maximo) = comando.maximo_por_execucao {
        opcoes.maximo_jobs_por_execucao = maximo;
    }
    opcoes.exigir_marcador_pronto = !comando.sem_marcador_pronto;
    opcoes.recuperar_jobs_em_processamento = comando.recuperar_processamento;

    if !comando.observar {
        let resumo = run_spool(&comando.raiz_spool, &comando.raiz_saida, &opcoes)?;
        return imprimir_json(&resumo);
    }

    let continuar = Arc::new(AtomicBool::new(true));
    let sinal = Arc::clone(&continuar);
    ctrlc::set_handler(move || {
        sinal.store(false, Ordering::SeqCst);
    })
    .map_err(|erro| {
        base64_file_converter::ConverterError::Spool(format!(
            "não foi possível instalar handler de encerramento: {erro}"
        ))
    })?;

    while continuar.load(Ordering::SeqCst) {
        let resumo = run_spool(&comando.raiz_spool, &comando.raiz_saida, &opcoes)?;
        imprimir_json(&resumo)?;
        if continuar.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(comando.intervalo_ms));
        }
    }
    Ok(())
}

/// Serializa um valor em uma única saída JSON legível.
fn imprimir_json<T: Serialize>(valor: &T) -> base64_file_converter::Result<()> {
    let saida = io::stdout();
    let mut bloqueio = saida.lock();
    serde_json::to_writer_pretty(&mut bloqueio, valor)?;
    use std::io::Write;
    bloqueio
        .write_all(b"\n")
        .map_err(|origem| base64_file_converter::ConverterError::io("stdout", origem))
}

/// Constrói a declaração de capacidades e limitações do executável.
fn capacidades() -> Capacidades {
    Capacidades {
        reconstrucao: "all byte sequences represented by valid Base64",
        usa_streaming: true,
        escrita_atomica: true,
        integridade: ["size", "sha256"],
        containers_recursivos: ["eml", "mht", "mhtml", "msg", "tnef", "zip", "7z", "rar"],
        preservados_sem_extrator_recursivo: ["pst", "ost", "mbox", "dbx", "olm"],
        documentos_office_preservados: true,
        formatos_desconhecidos: "preserved as application/octet-stream with an audit warning",
        acionadores: ["direct_cli", "stdin", "job_manifest", "directory_spool"],
        limites_seguranca: [
            "maximum recursion depth",
            "maximum extracted file count",
            "maximum single and total extracted bytes",
            "maximum compression ratio",
            "path traversal and symbolic-link rejection",
        ],
    }
}
