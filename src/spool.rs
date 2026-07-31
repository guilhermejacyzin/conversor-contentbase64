use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::error::{ConverterError, Result};
use crate::job::{JobResult, JobStatus, process_job_directory};

/// Configura uma varredura do adaptador de fila baseado em diretórios.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpoolOptions {
    /// Quantidade máxima de jobs processados simultaneamente.
    #[serde(rename = "max_concurrent_jobs", default = "concorrencia_padrao")]
    pub maximo_jobs_concorrentes: usize,
    /// Quantidade máxima de jobs reivindicados em uma varredura.
    #[serde(rename = "max_jobs_per_run", default = "maximo_jobs_padrao")]
    pub maximo_jobs_por_execucao: usize,
    /// Exige arquivo `READY` antes de reivindicar o diretório.
    #[serde(rename = "require_ready_marker", default = "padrao_verdadeiro")]
    pub exigir_marcador_pronto: bool,
    /// Direciona resultados parciais para quarentena operacional.
    #[serde(rename = "quarantine_partial_jobs", default = "padrao_verdadeiro")]
    pub colocar_parciais_em_quarentena: bool,
    /// Reprocessa jobs abandonados em `processing`; deve ser usado após validar que não há outro worker.
    #[serde(rename = "recover_processing_jobs", default)]
    pub recuperar_jobs_em_processamento: bool,
}

/// Fornece limites adequados para um worker local sem assumir hardware específico.
impl Default for SpoolOptions {
    /// Cria a configuração padrão de concorrência, marcador e recuperação.
    fn default() -> Self {
        Self {
            maximo_jobs_concorrentes: concorrencia_padrao(),
            maximo_jobs_por_execucao: maximo_jobs_padrao(),
            exigir_marcador_pronto: true,
            colocar_parciais_em_quarentena: true,
            recuperar_jobs_em_processamento: false,
        }
    }
}

/// Resume uma varredura sem expor conteúdo ou metadados sensíveis dos anexos.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpoolRunSummary {
    /// Jobs encontrados e reivindicados com sucesso.
    #[serde(rename = "claimed")]
    pub reivindicados: usize,
    /// Jobs concluídos integralmente.
    #[serde(rename = "completed")]
    pub concluidos: usize,
    /// Jobs finalizados com avisos, mas aceitos pela política.
    #[serde(rename = "partial")]
    pub parciais: usize,
    /// Jobs encaminhados à quarentena.
    #[serde(rename = "quarantined")]
    pub em_quarentena: usize,
    /// Jobs com erro fatal de manifesto, I/O ou contrato.
    #[serde(rename = "failed")]
    pub falhos: usize,
}

/// Representa o destino final de um diretório processado.
enum DestinoJob {
    Concluido,
    Quarentena,
    Falho,
}

/// Consolida o resultado interno de um job para cálculo do resumo.
struct ResultadoSpool {
    estado: Option<JobStatus>,
    destino: DestinoJob,
}

/// Executa uma única varredura do spool e processa jobs em paralelo.
///
/// O produtor pode ser API, mensageria, scheduler ou qualquer outro gatilho
/// capaz de criar `job.json` e suas fontes. A reivindicação usa renomeação de
/// diretório, impedindo que dois workers processem o mesmo item da `inbox`.
pub fn run_spool(
    raiz_spool: &Path,
    raiz_saida: &Path,
    opcoes: &SpoolOptions,
) -> Result<SpoolRunSummary> {
    validar_opcoes(opcoes)?;
    let diretorios = preparar_diretorios(raiz_spool)?;
    fs::create_dir_all(raiz_saida).map_err(|origem| ConverterError::io(raiz_saida, origem))?;

    let mut reivindicados = if opcoes.recuperar_jobs_em_processamento {
        listar_jobs(
            &diretorios.processamento,
            false,
            opcoes.maximo_jobs_por_execucao,
        )?
    } else {
        Vec::new()
    };
    let capacidade_restante = opcoes
        .maximo_jobs_por_execucao
        .saturating_sub(reivindicados.len());
    if capacidade_restante > 0 {
        let candidatos = listar_jobs(
            &diretorios.entrada,
            opcoes.exigir_marcador_pronto,
            capacidade_restante,
        )?;
        for candidato in candidatos {
            if let Some(caminho) = reivindicar_job(&candidato, &diretorios.processamento)? {
                reivindicados.push(caminho);
            }
        }
    }

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(opcoes.maximo_jobs_concorrentes)
        .thread_name(|indice| format!("base64-job-{indice}"))
        .build()
        .map_err(|erro| ConverterError::Spool(format!("falha ao criar pool de jobs: {erro}")))?;
    let resultados = pool.install(|| {
        reivindicados
            .par_iter()
            .map(|caminho| processar_job_reivindicado(caminho, raiz_saida, &diretorios, opcoes))
            .collect::<Vec<_>>()
    });

    let mut resumo = SpoolRunSummary {
        reivindicados: reivindicados.len(),
        ..SpoolRunSummary::default()
    };
    for resultado in resultados {
        match resultado {
            Ok(resultado) => match resultado.destino {
                DestinoJob::Concluido => {
                    if resultado.estado == Some(JobStatus::Partial) {
                        resumo.parciais += 1;
                    } else {
                        resumo.concluidos += 1;
                    }
                }
                DestinoJob::Quarentena => resumo.em_quarentena += 1,
                DestinoJob::Falho => resumo.falhos += 1,
            },
            Err(_) => resumo.falhos += 1,
        }
    }
    Ok(resumo)
}

/// Processa um job já reivindicado, grava o resultado e move o diretório ao estado final.
fn processar_job_reivindicado(
    diretorio_job: &Path,
    raiz_saida: &Path,
    diretorios: &DiretoriosSpool,
    opcoes: &SpoolOptions,
) -> Result<ResultadoSpool> {
    match process_job_directory(diretorio_job, raiz_saida) {
        Ok(resultado) => {
            write_json_atomic(&diretorio_job.join("result.json"), &resultado)?;
            let destino = selecionar_destino(&resultado, opcoes);
            mover_para_destino(diretorio_job, diretorios, &destino)?;
            Ok(ResultadoSpool {
                estado: Some(resultado.estado),
                destino,
            })
        }
        Err(erro) => {
            let mensagem = sanitizar_erro(&erro.to_string());
            write_json_atomic(
                &diretorio_job.join("error.json"),
                &serde_json::json!({
                    "schema_version": "base64-file-converter.error.v1",
                    "error": mensagem,
                }),
            )?;
            mover_para_destino(diretorio_job, diretorios, &DestinoJob::Falho)?;
            Ok(ResultadoSpool {
                estado: None,
                destino: DestinoJob::Falho,
            })
        }
    }
}

/// Escolhe o estado de retenção com base no resultado e na política operacional.
fn selecionar_destino(resultado: &JobResult, opcoes: &SpoolOptions) -> DestinoJob {
    match resultado.estado {
        JobStatus::Completed => DestinoJob::Concluido,
        JobStatus::Partial if !opcoes.colocar_parciais_em_quarentena => DestinoJob::Concluido,
        JobStatus::Partial | JobStatus::Quarantined => DestinoJob::Quarentena,
        JobStatus::Failed => DestinoJob::Falho,
    }
}

/// Move o diretório completo, mantendo fontes, auditoria e resultado juntos.
fn mover_para_destino(
    diretorio_job: &Path,
    diretorios: &DiretoriosSpool,
    destino: &DestinoJob,
) -> Result<()> {
    let nome = diretorio_job.file_name().ok_or_else(|| {
        ConverterError::Spool(format!(
            "diretório de job sem nome: {}",
            diretorio_job.display()
        ))
    })?;
    let raiz_destino = match destino {
        DestinoJob::Concluido => &diretorios.concluidos,
        DestinoJob::Quarentena => &diretorios.quarentena,
        DestinoJob::Falho => &diretorios.falhos,
    };
    let caminho_destino = raiz_destino.join(nome);
    if caminho_destino.exists() {
        return Err(ConverterError::OutputConflict {
            caminho: caminho_destino,
        });
    }
    fs::rename(diretorio_job, &caminho_destino)
        .map_err(|origem| ConverterError::io(&caminho_destino, origem))
}

/// Reivindica atomicamente um candidato da `inbox`.
fn reivindicar_job(candidato: &Path, diretorio_processamento: &Path) -> Result<Option<PathBuf>> {
    let nome = candidato.file_name().ok_or_else(|| {
        ConverterError::Spool(format!("candidato sem nome: {}", candidato.display()))
    })?;
    let destino = diretorio_processamento.join(nome);
    match fs::rename(candidato, &destino) {
        Ok(()) => Ok(Some(destino)),
        Err(origem)
            if matches!(
                origem.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::AlreadyExists
            ) =>
        {
            Ok(None)
        }
        Err(origem) => Err(ConverterError::io(candidato, origem)),
    }
}

/// Lista somente diretórios regulares com manifesto e marcador opcional.
fn listar_jobs(diretorio: &Path, exigir_marcador: bool, limite: usize) -> Result<Vec<PathBuf>> {
    let mut caminhos = Vec::new();
    for entrada in
        fs::read_dir(diretorio).map_err(|origem| ConverterError::io(diretorio, origem))?
    {
        let entrada = entrada.map_err(|origem| ConverterError::io(diretorio, origem))?;
        let tipo = entrada
            .file_type()
            .map_err(|origem| ConverterError::io(entrada.path(), origem))?;
        if !tipo.is_dir() || tipo.is_symlink() {
            continue;
        }
        let caminho = entrada.path();
        if !caminho.join("job.json").is_file() {
            continue;
        }
        if exigir_marcador && !caminho.join("READY").is_file() {
            continue;
        }
        caminhos.push(caminho);
    }
    caminhos.sort();
    caminhos.truncate(limite);
    Ok(caminhos)
}

/// Escreve JSON em temporário, sincroniza e publica por renomeação.
pub fn write_json_atomic<T: Serialize>(caminho: &Path, valor: &T) -> Result<()> {
    let diretorio = caminho
        .parent()
        .ok_or_else(|| ConverterError::UnsafeRelativePath(caminho.display().to_string()))?;
    fs::create_dir_all(diretorio).map_err(|origem| ConverterError::io(diretorio, origem))?;
    let nome = caminho
        .file_name()
        .and_then(|valor| valor.to_str())
        .ok_or_else(|| ConverterError::UnsafeFilename(caminho.display().to_string()))?;
    let instante = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporario = diretorio.join(format!(".{nome}.{}.{}.part", std::process::id(), instante));
    let mut arquivo = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporario)
        .map_err(|origem| ConverterError::io(&temporario, origem))?;
    serde_json::to_writer_pretty(&mut arquivo, valor)?;
    arquivo
        .write_all(b"\n")
        .and_then(|_| arquivo.flush())
        .and_then(|_| arquivo.sync_all())
        .map_err(|origem| ConverterError::io(&temporario, origem))?;
    drop(arquivo);
    if caminho.exists() {
        let _ = fs::remove_file(&temporario);
        return Err(ConverterError::OutputConflict {
            caminho: caminho.to_path_buf(),
        });
    }
    fs::rename(&temporario, caminho).map_err(|origem| ConverterError::io(caminho, origem))
}

/// Agrupa os diretórios fixos do adaptador de spool.
struct DiretoriosSpool {
    entrada: PathBuf,
    processamento: PathBuf,
    concluidos: PathBuf,
    quarentena: PathBuf,
    falhos: PathBuf,
}

/// Cria os estados do spool sem remover conteúdo existente.
fn preparar_diretorios(raiz: &Path) -> Result<DiretoriosSpool> {
    let diretorios = DiretoriosSpool {
        entrada: raiz.join("inbox"),
        processamento: raiz.join("processing"),
        concluidos: raiz.join("completed"),
        quarentena: raiz.join("quarantine"),
        falhos: raiz.join("failed"),
    };
    for caminho in [
        &diretorios.entrada,
        &diretorios.processamento,
        &diretorios.concluidos,
        &diretorios.quarentena,
        &diretorios.falhos,
    ] {
        fs::create_dir_all(caminho).map_err(|origem| ConverterError::io(caminho, origem))?;
    }
    Ok(diretorios)
}

/// Rejeita configurações que impediriam progresso ou consumiriam recursos sem limite.
fn validar_opcoes(opcoes: &SpoolOptions) -> Result<()> {
    if opcoes.maximo_jobs_concorrentes == 0 {
        return Err(ConverterError::Spool(
            "max_concurrent_jobs deve ser maior que zero".to_string(),
        ));
    }
    if opcoes.maximo_jobs_por_execucao == 0 {
        return Err(ConverterError::Spool(
            "max_jobs_per_run deve ser maior que zero".to_string(),
        ));
    }
    Ok(())
}

/// Remove quebras de linha e limita mensagens persistidas em `error.json`.
fn sanitizar_erro(mensagem: &str) -> String {
    mensagem
        .replace(['\r', '\n'], " ")
        .chars()
        .take(2_000)
        .collect()
}

/// Calcula concorrência conservadora com base no paralelismo disponível.
fn concorrencia_padrao() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(2)
        .clamp(1, 16)
}

/// Limita uma varredura a mil jobs para evitar reivindicação irrestrita.
fn maximo_jobs_padrao() -> usize {
    1_000
}

/// Retorna `true` para barreiras operacionais habilitadas por padrão.
fn padrao_verdadeiro() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirma que configurações sem concorrência são rejeitadas.
    #[test]
    fn rejeita_concorrencia_zero() {
        let opcoes = SpoolOptions {
            maximo_jobs_concorrentes: 0,
            ..SpoolOptions::default()
        };
        assert!(validar_opcoes(&opcoes).is_err());
    }

    /// Confirma que todos os estados do spool são materializados.
    #[test]
    fn prepara_estados_spool() {
        let temporario = tempfile::tempdir().unwrap();
        let diretorios = preparar_diretorios(temporario.path()).unwrap();
        assert!(diretorios.entrada.is_dir());
        assert!(diretorios.quarentena.is_dir());
    }
}
