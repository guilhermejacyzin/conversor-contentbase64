use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};

use crate::error::{ConverterError, Result};

const MAXIMO_CARACTERES_SEGMENTO: usize = 180;
const NOMES_RESERVADOS_WINDOWS: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Valida um caminho relativo fornecido por manifesto ou contêiner.
///
/// Rejeita caminhos absolutos, prefixos de unidade, `..`, caracteres inválidos,
/// nomes reservados e segmentos excessivamente longos.
pub(crate) fn validate_relative_path(caminho: &Path) -> Result<PathBuf> {
    if caminho.as_os_str().is_empty() || caminho.is_absolute() {
        return Err(ConverterError::UnsafeRelativePath(
            caminho.display().to_string(),
        ));
    }

    let mut validado = PathBuf::new();
    for componente in caminho.components() {
        match componente {
            Component::Normal(segmento) => {
                let segmento = segmento.to_string_lossy();
                validar_segmento_caminho(&segmento)?;
                validado.push(segmento.as_ref());
            }
            _ => {
                return Err(ConverterError::UnsafeRelativePath(
                    caminho.display().to_string(),
                ));
            }
        }
    }
    Ok(validado)
}

/// Valida um nome lógico que não pode conter componentes de diretório.
pub(crate) fn validate_filename(nome_arquivo: &str) -> Result<()> {
    validar_segmento_caminho(nome_arquivo)
        .map_err(|_| ConverterError::UnsafeFilename(nome_arquivo.to_string()))
}

/// Sanitiza um nome não confiável sem perder sua extensão útil.
///
/// A função remove qualquer diretório declarado, substitui caracteres inválidos,
/// protege nomes reservados do Windows e limita o tamanho com sufixo hash.
pub(crate) fn sanitize_filename(nome_arquivo: &str, nome_alternativo: &str) -> String {
    let normalizado = nome_arquivo.replace('\\', "/");
    let nome_base = normalizado.rsplit('/').next().unwrap_or_default().trim();
    let mut sanitizado: String = nome_base
        .chars()
        .map(|caractere| {
            if caractere.is_control()
                || matches!(
                    caractere,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '_'
            } else {
                caractere
            }
        })
        .collect();
    sanitizado = sanitizado.trim_end_matches([' ', '.']).to_string();
    if sanitizado.is_empty() {
        sanitizado = nome_alternativo.to_string();
    }

    let radical = sanitizado
        .split('.')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    if NOMES_RESERVADOS_WINDOWS.contains(&radical.as_str()) {
        sanitizado.insert(0, '_');
    }

    if sanitizado.chars().count() > MAXIMO_CARACTERES_SEGMENTO {
        let mut calculador = DefaultHasher::new();
        sanitizado.hash(&mut calculador);
        let sufixo = format!("_{:016x}", calculador.finish());
        let extensao = Path::new(&sanitizado)
            .extension()
            .and_then(|valor| valor.to_str())
            .map(|valor| format!(".{valor}"))
            .unwrap_or_default();
        let quantidade_preservada = MAXIMO_CARACTERES_SEGMENTO
            .saturating_sub(sufixo.chars().count())
            .saturating_sub(extensao.chars().count());
        let prefixo: String = sanitizado.chars().take(quantidade_preservada).collect();
        sanitizado = format!("{prefixo}{sufixo}{extensao}");
    }
    sanitizado
}

/// Valida um único segmento de caminho de forma compatível com Windows e Unix.
fn validar_segmento_caminho(segmento: &str) -> Result<()> {
    if segmento.is_empty()
        || segmento == "."
        || segmento == ".."
        || segmento.chars().count() > MAXIMO_CARACTERES_SEGMENTO
        || segmento.ends_with(' ')
        || segmento.ends_with('.')
        || segmento.chars().any(|caractere| {
            caractere.is_control()
                || matches!(
                    caractere,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
    {
        return Err(ConverterError::UnsafeRelativePath(segmento.to_string()));
    }

    let radical = segmento
        .split('.')
        .next()
        .unwrap_or(segmento)
        .trim()
        .to_ascii_uppercase();
    if NOMES_RESERVADOS_WINDOWS.contains(&radical.as_str()) {
        return Err(ConverterError::UnsafeRelativePath(segmento.to_string()));
    }
    Ok(())
}

/// Resolve uma fonte relativa e confirma que o caminho real continua dentro do job.
///
/// A canonicalização também impede escapes por junctions e links simbólicos.
pub(crate) fn canonical_source_within(
    raiz_trabalho: &Path,
    caminho_relativo: &Path,
) -> Result<PathBuf> {
    let caminho_relativo = validate_relative_path(caminho_relativo)?;
    let raiz_canonica = std::fs::canonicalize(raiz_trabalho)
        .map_err(|origem| ConverterError::io(raiz_trabalho, origem))?;
    let candidato = raiz_canonica.join(caminho_relativo);
    let caminho_canonico = std::fs::canonicalize(&candidato)
        .map_err(|origem| ConverterError::io(&candidato, origem))?;
    if !caminho_canonico.starts_with(&raiz_canonica) {
        return Err(ConverterError::SourcePathEscape(
            candidato.display().to_string(),
        ));
    }
    Ok(caminho_canonico)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirma que um caminho relativo aninhado e seguro é aceito.
    #[test]
    fn aceita_caminho_relativo_seguro() {
        assert_eq!(
            validate_relative_path(Path::new("documents/contract.pdf")).unwrap(),
            PathBuf::from("documents/contract.pdf")
        );
    }

    /// Confirma a rejeição de travessia, nome reservado e caractere inválido.
    #[test]
    fn rejeita_travessia_e_nomes_reservados() {
        assert!(validate_relative_path(Path::new("../secret.pdf")).is_err());
        assert!(validate_relative_path(Path::new("CON.txt")).is_err());
        assert!(validate_relative_path(Path::new("folder/file?.pdf")).is_err());
    }

    /// Confirma a sanitização previsível de nomes recebidos de fontes externas.
    #[test]
    fn sanitiza_nomes_de_anexo_nao_confiaveis() {
        assert_eq!(
            sanitize_filename("../../CON?.pdf", "attachment.bin"),
            "CON_.pdf"
        );
        assert_eq!(sanitize_filename("", "attachment.bin"), "attachment.bin");
    }
}
