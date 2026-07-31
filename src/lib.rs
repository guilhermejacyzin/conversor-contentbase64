//! Conversor portátil de conteúdo Base64 para arquivos originais.
//!
//! O crate oferece decodificação em streaming, publicação atômica, SHA-256,
//! idempotência, validação de formato, expansão recursiva controlada, auditoria
//! e um adaptador de fila baseado em diretórios. O núcleo não contém regras de
//! negócio nem caminhos fixos e pode ser incorporado em APIs, workers ou CLIs.

mod decoder;
mod error;
mod expansion;
mod job;
mod media;
mod path_policy;
mod spool;

pub use decoder::{
    Base64Variant, DecodeOptions, DecodeResult, decode_base64_reader_to_file, decode_base64_to_file,
};
pub use error::{ConverterError, Result};
pub use expansion::{
    ContainerCapability, ExpandedFile, ExpansionPolicy, ExpansionResult, ExpansionWarning,
    expand_containers,
};
pub use job::{
    AttachmentJob, AttachmentRequest, AttachmentResult, AttachmentStatus, JobPolicy, JobResult,
    JobStatus, process_job_directory,
};
pub use media::{MediaInspection, SignatureStatus, detect_media_type, validate_file_signature};
pub use spool::{SpoolOptions, SpoolRunSummary, run_spool, write_json_atomic};
