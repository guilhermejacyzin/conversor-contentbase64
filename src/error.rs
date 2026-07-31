use std::path::PathBuf;

/// Erros tipados do conversor, adequados para auditoria e tratamento por adaptadores.
#[derive(Debug, thiserror::Error)]
pub enum ConverterError {
    /// Falha de leitura, escrita, sincronização ou movimentação.
    #[error("I/O error at {caminho}: {origem}")]
    Io {
        /// Caminho técnico relacionado à falha.
        caminho: PathBuf,
        /// Erro original do sistema operacional.
        #[source]
        origem: std::io::Error,
    },

    /// Conteúdo não obedece ao alfabeto Base64 configurado.
    #[error("invalid Base64 payload: {0}")]
    InvalidBase64(String),

    /// Arquivo reconstruído excede o limite individual.
    #[error("decoded file exceeds the configured limit of {limite_bytes} bytes")]
    SizeLimitExceeded {
        /// Limite aplicado no momento da falha.
        limite_bytes: u64,
    },

    /// Tamanho calculado diverge do tamanho esperado.
    #[error("decoded size mismatch: expected {esperado} bytes, got {atual}")]
    SizeMismatch {
        /// Tamanho declarado pela origem.
        esperado: u64,
        /// Tamanho efetivamente calculado.
        atual: u64,
    },

    /// SHA-256 calculado diverge do valor esperado.
    #[error("SHA-256 mismatch: expected {esperado}, got {atual}")]
    HashMismatch {
        /// Hash declarado pela origem.
        esperado: String,
        /// Hash efetivamente calculado.
        atual: String,
    },

    /// Caminho relativo contém elemento inseguro.
    #[error("unsafe relative path: {0}")]
    UnsafeRelativePath(String),

    /// Fonte canonicalizada escaparia do diretório do trabalho.
    #[error("source path escapes the job directory: {0}")]
    SourcePathEscape(String),

    /// Nome lógico não é seguro para uso como arquivo.
    #[error("unsupported or unsafe filename: {0}")]
    UnsafeFilename(String),

    /// Extensão, assinatura ou MIME apresentam divergência.
    #[error("file signature validation failed for {nome_arquivo}: {motivo}")]
    SignatureMismatch {
        /// Nome lógico usado na validação.
        nome_arquivo: String,
        /// Explicação da divergência.
        motivo: String,
    },

    /// Destino existente contém bytes diferentes.
    #[error("output conflict at {caminho}; an existing file has different content")]
    OutputConflict {
        /// Caminho que não pode ser sobrescrito.
        caminho: PathBuf,
    },

    /// Manifesto viola o contrato de entrada.
    #[error("invalid job manifest: {0}")]
    InvalidJob(String),

    /// JSON de entrada ou saída é inválido.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Adaptador de spool encontrou estado operacional inválido.
    #[error("spool error: {0}")]
    Spool(String),

    /// Contêiner conhecido não pôde ser interpretado.
    #[error("container processing error for {caminho}: {motivo}")]
    Container {
        /// Arquivo contêiner relacionado.
        caminho: PathBuf,
        /// Explicação fornecida pelo extrator.
        motivo: String,
    },

    /// Limite de profundidade, quantidade, volume ou taxa foi alcançado.
    #[error("recursive extraction limit exceeded: {0}")]
    ExpansionLimit(String),
}

/// Implementa construtores comuns de erro.
impl ConverterError {
    /// Constrói um erro de I/O preservando o caminho técnico relacionado.
    pub fn io(caminho: impl Into<PathBuf>, origem: std::io::Error) -> Self {
        Self::Io {
            caminho: caminho.into(),
            origem,
        }
    }
}

/// Resultado padrão utilizado por todas as operações do componente.
pub type Result<T> = std::result::Result<T, ConverterError>;
