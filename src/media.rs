use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{ConverterError, Result};

const ASSINATURA_ZIP: &[u8] = b"PK\x03\x04";
const ASSINATURA_OLE: &[u8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1";

/// Classifica o nível de confiança obtido durante a inspeção do arquivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureStatus {
    /// A assinatura binária confere com a extensão declarada.
    SignatureMatched,
    /// O formato não possui uma assinatura forte e foi validado por estrutura básica.
    StructuralMatch,
    /// A extensão é conhecida, mas só pode ser classificada sem confirmação binária forte.
    ExtensionOnly,
    /// A extensão não está no catálogo; os bytes foram preservados sem bloqueio.
    UnknownPreserved,
    /// A validação foi explicitamente desabilitada pelo chamador.
    ValidationDisabled,
}

/// Resultado auditável da identificação de tipo e validação de assinatura.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaInspection {
    /// Tipo MIME resolvido para persistência e integração.
    #[serde(rename = "media_type")]
    pub tipo_mime: String,
    /// Extensão normalizada, sem ponto e em letras minúsculas.
    #[serde(rename = "extension")]
    pub extensao: String,
    /// Nível de confiança da identificação.
    #[serde(rename = "signature_status")]
    pub estado_assinatura: SignatureStatus,
    /// Tipo MIME inferido exclusivamente pelos bytes, quando disponível.
    #[serde(rename = "detected_by_signature")]
    pub detectado_por_assinatura: Option<String>,
    /// Aviso não bloqueante para formatos preservados sem validação forte.
    #[serde(rename = "warning")]
    pub aviso: Option<String>,
}

/// Detecta um tipo MIME por assinatura binária sem confiar no nome do arquivo.
///
/// Complementa a biblioteca `infer` com assinaturas necessárias para contêineres
/// corporativos e formatos legados que compartilham estruturas ZIP ou OLE.
pub fn detect_media_type(prefixo: &[u8]) -> Option<&'static str> {
    if prefixo.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else if prefixo.starts_with(ASSINATURA_OLE) {
        Some("application/x-ole-storage")
    } else if prefixo.starts_with(ASSINATURA_ZIP)
        || prefixo.starts_with(b"PK\x05\x06")
        || prefixo.starts_with(b"PK\x07\x08")
    {
        Some("application/zip")
    } else if prefixo.starts_with(b"7z\xBC\xAF\x27\x1C") {
        Some("application/x-7z-compressed")
    } else if prefixo.starts_with(b"Rar!\x1A\x07") {
        Some("application/vnd.rar")
    } else if prefixo.starts_with(b"{\\rtf") {
        Some("application/rtf")
    } else if prefixo.starts_with(b"!BDN") {
        Some("application/vnd.ms-outlook-pst")
    } else if prefixo.starts_with(b"\x78\x9F\x3E\x22") {
        Some("application/ms-tnef")
    } else {
        infer::get(prefixo).map(|tipo| tipo.mime_type())
    }
}

/// Valida a coerência entre extensão, assinatura, estrutura e MIME esperado.
///
/// Formatos conhecidos com assinatura forte divergente são rejeitados. Formatos
/// desconhecidos são preservados quando `preservar_desconhecidos` está habilitado,
/// garantindo que a ausência de um analisador não provoque perda do anexo.
pub fn validate_file_signature(
    nome_arquivo: &str,
    prefixo: &[u8],
    cauda: &[u8],
    tipo_mime_esperado: Option<&str>,
    preservar_desconhecidos: bool,
) -> Result<MediaInspection> {
    let extensao = Path::new(nome_arquivo)
        .extension()
        .and_then(|valor| valor.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let detectado = detect_media_type(prefixo);
    let tipo_mime = media_type_for_extension(&extensao, detectado);
    let conhecido = extension_is_known(&extensao);

    let estado = if is_textual_extension(&extensao) {
        if looks_like_text(prefixo) {
            SignatureStatus::StructuralMatch
        } else {
            return Err(erro_assinatura(
                nome_arquivo,
                &extensao,
                detectado,
                "o conteúdo não apresenta uma codificação textual reconhecível",
            ));
        }
    } else if matches!(extensao.as_str(), "pdf" | "pdfa") {
        if detectado == Some("application/pdf") && contains_pdf_eof(cauda) {
            SignatureStatus::SignatureMatched
        } else {
            return Err(erro_assinatura(
                nome_arquivo,
                &extensao,
                detectado,
                "cabeçalho PDF ou marcador final %%EOF ausente",
            ));
        }
    } else if let Some(assinaturas_aceitas) = accepted_signatures(&extensao) {
        if detectado.is_some_and(|valor| assinaturas_aceitas.contains(&valor)) {
            SignatureStatus::SignatureMatched
        } else {
            return Err(erro_assinatura(
                nome_arquivo,
                &extensao,
                detectado,
                "a assinatura binária não corresponde à extensão",
            ));
        }
    } else if conhecido {
        SignatureStatus::ExtensionOnly
    } else if preservar_desconhecidos {
        SignatureStatus::UnknownPreserved
    } else {
        return Err(erro_assinatura(
            nome_arquivo,
            &extensao,
            detectado,
            "a extensão não está no catálogo e a preservação foi desabilitada",
        ));
    };

    if let Some(esperado) = tipo_mime_esperado
        && !esperado.trim().is_empty()
        && !media_types_compatible(esperado, tipo_mime)
    {
        return Err(ConverterError::SignatureMismatch {
            nome_arquivo: nome_arquivo.to_string(),
            motivo: format!(
                "tipo MIME esperado {}, tipo resolvido {}",
                esperado.trim(),
                tipo_mime
            ),
        });
    }

    let aviso = match estado {
        SignatureStatus::ExtensionOnly => Some(
            "formato conhecido sem assinatura forte; extensão preservada e registrada".to_string(),
        ),
        SignatureStatus::UnknownPreserved => Some(
            "formato fora do catálogo; bytes preservados sem tentativa de interpretação"
                .to_string(),
        ),
        _ => None,
    };

    Ok(MediaInspection {
        tipo_mime: tipo_mime.to_string(),
        extensao,
        estado_assinatura: estado,
        detectado_por_assinatura: detectado.map(str::to_string),
        aviso,
    })
}

/// Constrói um erro consistente de divergência de assinatura.
fn erro_assinatura(
    nome_arquivo: &str,
    extensao: &str,
    detectado: Option<&str>,
    detalhe: &str,
) -> ConverterError {
    ConverterError::SignatureMismatch {
        nome_arquivo: nome_arquivo.to_string(),
        motivo: format!(
            "extensão .{}; assinatura detectada {:?}; {}",
            extensao, detectado, detalhe
        ),
    }
}

/// Verifica se a parte final do arquivo contém o marcador obrigatório de encerramento PDF.
fn contains_pdf_eof(cauda: &[u8]) -> bool {
    cauda.windows(5).any(|janela| janela == b"%%EOF")
}

/// Reconhece texto UTF-8, UTF-16 com BOM e arquivos textuais vazios.
fn looks_like_text(prefixo: &[u8]) -> bool {
    prefixo.is_empty()
        || prefixo.starts_with(b"\xEF\xBB\xBF")
        || prefixo.starts_with(b"\xFF\xFE")
        || prefixo.starts_with(b"\xFE\xFF")
        || (!prefixo.contains(&0) && std::str::from_utf8(prefixo).is_ok())
}

/// Informa se uma extensão utiliza validação textual em vez de assinatura binária.
fn is_textual_extension(extensao: &str) -> bool {
    matches!(
        extensao,
        "txt"
            | "text"
            | "csv"
            | "tsv"
            | "json"
            | "jsonl"
            | "xml"
            | "xsd"
            | "xsl"
            | "xslt"
            | "yaml"
            | "yml"
            | "html"
            | "htm"
            | "eml"
            | "emlx"
            | "mht"
            | "mhtml"
            | "mbox"
            | "ics"
            | "vcf"
            | "sql"
            | "log"
            | "md"
    )
}

/// Retorna as assinaturas binárias aceitas para extensões com verificação forte.
fn accepted_signatures(extensao: &str) -> Option<&'static [&'static str]> {
    match extensao {
        "jpg" | "jpeg" | "jpe" | "jfif" => Some(&["image/jpeg"]),
        "png" => Some(&["image/png"]),
        "gif" => Some(&["image/gif"]),
        "tif" | "tiff" => Some(&["image/tiff"]),
        "bmp" | "dib" => Some(&["image/bmp"]),
        "webp" => Some(&["image/webp"]),
        "heic" | "heif" => Some(&["image/heic", "image/heif"]),
        "zip" => Some(&["application/zip"]),
        "docx" | "docm" | "dotx" | "dotm" | "xlsx" | "xlsm" | "xlsb" | "xltx" | "xltm" | "xlam"
        | "pptx" | "pptm" | "potx" | "potm" | "ppsx" | "ppsm" | "ppam" | "odt" | "ods" | "odp"
        | "vsdx" | "vsdm" => Some(&["application/zip"]),
        "doc" | "dot" | "xls" | "xlt" | "xla" | "ppt" | "pot" | "pps" | "ppa" | "msg" | "oft"
        | "pub" | "vsd" | "vss" | "vst" => Some(&["application/x-ole-storage"]),
        "rar" => Some(&["application/vnd.rar"]),
        "7z" => Some(&["application/x-7z-compressed"]),
        "gz" | "tgz" => Some(&["application/gzip"]),
        "bz2" | "tbz" | "tbz2" => Some(&["application/x-bzip2"]),
        "xz" | "txz" => Some(&["application/x-xz"]),
        "rtf" => Some(&["application/rtf"]),
        "pst" | "ost" => Some(&["application/vnd.ms-outlook-pst"]),
        _ => None,
    }
}

/// Informa se a extensão pertence ao catálogo de formatos preserváveis conhecidos.
fn extension_is_known(extensao: &str) -> bool {
    is_textual_extension(extensao)
        || accepted_signatures(extensao).is_some()
        || matches!(
            extensao,
            "xlsb"
                | "pdfa"
                | "svg"
                | "ico"
                | "emf"
                | "wmf"
                | "raw"
                | "tar"
                | "cab"
                | "iso"
                | "dat"
                | "one"
                | "onepkg"
                | "rpmsg"
                | "emlx"
                | "pst"
                | "ost"
                | "mbx"
                | "dbx"
                | "olm"
                | "vcf"
                | "ics"
        )
}

/// Resolve o tipo MIME preferindo a semântica da extensão para formatos compostos.
fn media_type_for_extension(extensao: &str, detectado: Option<&'static str>) -> &'static str {
    match extensao {
        "doc" | "dot" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "docm" => "application/vnd.ms-word.document.macroenabled.12",
        "dotx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.template",
        "dotm" => "application/vnd.ms-word.template.macroenabled.12",
        "xls" | "xlt" | "xla" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsm" => "application/vnd.ms-excel.sheet.macroenabled.12",
        "xlsb" => "application/vnd.ms-excel.sheet.binary.macroenabled.12",
        "xltx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.template",
        "xltm" => "application/vnd.ms-excel.template.macroenabled.12",
        "xlam" => "application/vnd.ms-excel.addin.macroenabled.12",
        "ppt" | "pot" | "pps" | "ppa" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "pptm" => "application/vnd.ms-powerpoint.presentation.macroenabled.12",
        "potx" => "application/vnd.openxmlformats-officedocument.presentationml.template",
        "potm" => "application/vnd.ms-powerpoint.template.macroenabled.12",
        "ppsx" => "application/vnd.openxmlformats-officedocument.presentationml.slideshow",
        "ppsm" => "application/vnd.ms-powerpoint.slideshow.macroenabled.12",
        "ppam" => "application/vnd.ms-powerpoint.addin.macroenabled.12",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "msg" | "oft" => "application/vnd.ms-outlook",
        "eml" | "emlx" => "message/rfc822",
        "mht" | "mhtml" => "multipart/related",
        "pst" | "ost" => "application/vnd.ms-outlook-pst",
        "mbox" | "mbx" => "application/mbox",
        "txt" | "text" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "json" | "jsonl" => "application/json",
        "xml" | "xsd" | "xsl" | "xslt" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "html" | "htm" => "text/html",
        "rtf" => "application/rtf",
        "rar" => "application/vnd.rar",
        "7z" => "application/x-7z-compressed",
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "dat" if detectado == Some("application/ms-tnef") => "application/ms-tnef",
        _ => detectado.unwrap_or("application/octet-stream"),
    }
}

/// Compara tipos MIME tolerando apenas generalizações explicitamente seguras.
fn media_types_compatible(esperado: &str, resolvido: &str) -> bool {
    let esperado = esperado.trim().to_ascii_lowercase();
    esperado == resolvido
        || esperado == "application/octet-stream"
        || (esperado == "application/zip"
            && (resolvido.starts_with("application/vnd.openxmlformats")
                || resolvido.starts_with("application/vnd.ms-")
                || resolvido.starts_with("application/vnd.oasis")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirma que um PDF exige cabeçalho e marcador final.
    #[test]
    fn valida_cabecalho_e_rodape_pdf() {
        let prefixo = b"%PDF-1.7\n";
        let cauda = b"endobj\n%%EOF\n";
        let resultado = validate_file_signature(
            "contrato.pdf",
            prefixo,
            cauda,
            Some("application/pdf"),
            true,
        )
        .unwrap();
        assert_eq!(resultado.tipo_mime, "application/pdf");
        assert_eq!(
            resultado.estado_assinatura,
            SignatureStatus::SignatureMatched
        );
    }

    /// Confirma que conteúdo conhecido com extensão falsa é rejeitado.
    #[test]
    fn rejeita_divergencia_entre_extensao_e_assinatura() {
        assert!(
            validate_file_signature("contrato.pdf", b"\x89PNG\r\n\x1A\n", b"dados", None, true,)
                .is_err()
        );
    }

    /// Confirma que formatos desconhecidos são preservados com aviso auditável.
    #[test]
    fn preserva_formato_desconhecido() {
        let resultado =
            validate_file_signature("evidencia.xyzcorp", b"\x01\x02\x03", b"", None, true).unwrap();
        assert_eq!(
            resultado.estado_assinatura,
            SignatureStatus::UnknownPreserved
        );
    }
}
