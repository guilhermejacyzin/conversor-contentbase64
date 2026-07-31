//! Testes ponta a ponta sobre contratos, segurança, integridade e recursão.

use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use base64_file_converter::{
    AttachmentStatus, JobResult, JobStatus, SignatureStatus, process_job_directory,
};
use serde_json::{Value, json};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

/// Cria um manifesto mínimo e genérico para um único anexo inline.
fn criar_manifesto(
    diretorio_job: &Path,
    nome_arquivo: &str,
    caminho_saida: &str,
    conteudo_original: &[u8],
) {
    fs::create_dir_all(diretorio_job).unwrap();
    let manifesto = json!({
        "schema_version": "base64-file-converter.job.v1",
        "job_id": "job-integration-001",
        "correlation_id": "correlation-integration-001",
        "source_system": "sample-source",
        "flow": "document-ingestion",
        "operation_id": "get-document",
        "business_keys": {
            "record_id": "record-001"
        },
        "attachments": [{
            "attachment_id": "attachment-001",
            "filename": nome_arquivo,
            "content_base64": STANDARD.encode(conteudo_original),
            "output_relative_path": caminho_saida
        }]
    });
    fs::write(
        diretorio_job.join("job.json"),
        serde_json::to_vec_pretty(&manifesto).unwrap(),
    )
    .unwrap();
}

/// Retorna um PDF mínimo suficiente para validar cabeçalho, conteúdo e rodapé.
fn pdf_minimo() -> Vec<u8> {
    b"%PDF-1.4\n1 0 obj\n<<>>\nendobj\ntrailer\n<<>>\n%%EOF\n".to_vec()
}

/// Monta um EML que contém outro EML com um PDF no nível seguinte.
fn email_aninhado() -> Vec<u8> {
    let pdf_base64 = STANDARD.encode(pdf_minimo());
    let email_interno = format!(
        "From: sender@example.test\r\n\
         To: receiver@example.test\r\n\
         Subject: Nested message\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"inner-boundary\"\r\n\
         \r\n\
         --inner-boundary\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         Nested body\r\n\
         --inner-boundary\r\n\
         Content-Type: application/pdf; name=\"evidence.pdf\"\r\n\
         Content-Disposition: attachment; filename=\"evidence.pdf\"\r\n\
         Content-Transfer-Encoding: base64\r\n\
         \r\n\
         {pdf_base64}\r\n\
         --inner-boundary--\r\n"
    );
    format!(
        "From: sender@example.test\r\n\
         To: receiver@example.test\r\n\
         Subject: Root message\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"outer-boundary\"\r\n\
         \r\n\
         --outer-boundary\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         Root body\r\n\
         --outer-boundary\r\n\
         Content-Type: message/rfc822; name=\"nested.eml\"\r\n\
         Content-Disposition: attachment; filename=\"nested.eml\"\r\n\
         \r\n\
         {email_interno}\r\n\
         --outer-boundary--\r\n"
    )
    .into_bytes()
}

/// Cria um ZIP em memória com o nome de entrada solicitado.
fn criar_zip(nome_entrada: &str, conteudo: &[u8]) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut escritor = ZipWriter::new(cursor);
    escritor
        .start_file(nome_entrada, SimpleFileOptions::default())
        .unwrap();
    escritor.write_all(conteudo).unwrap();
    escritor.finish().unwrap().into_inner()
}

/// Localiza um arquivo materializado pelo caminho portátil registrado no resultado.
fn ler_arquivo_resultado(raiz_saida: &Path, caminho_portatil: &str) -> Vec<u8> {
    let caminho = caminho_portatil
        .split('/')
        .fold(raiz_saida.to_path_buf(), |acumulado, parte| {
            acumulado.join(parte)
        });
    fs::read(caminho).unwrap()
}

/// Confirma recursão EML -> EML -> PDF com bytes e linhagem preservados.
#[test]
fn extrai_email_dentro_de_email_recursivamente() {
    let temporario = tempfile::tempdir().unwrap();
    let diretorio_job = temporario.path().join("job");
    let raiz_saida = temporario.path().join("output");
    criar_manifesto(
        &diretorio_job,
        "root.eml",
        "documents/root.eml",
        &email_aninhado(),
    );

    let resultado = process_job_directory(&diretorio_job, &raiz_saida).unwrap();
    assert_eq!(resultado.estado, JobStatus::Completed);
    let anexo = &resultado.anexos[0];
    assert_eq!(anexo.estado, AttachmentStatus::Completed);
    let expansao = anexo.expansao.as_ref().unwrap();
    assert_eq!(expansao.quantidade_arquivos_extraidos, 2);

    let arquivo_pdf = expansao
        .arquivos
        .iter()
        .find(|arquivo| arquivo.nome_original == "evidence.pdf")
        .unwrap();
    assert_eq!(
        arquivo_pdf.inspecao_midia.estado_assinatura,
        SignatureStatus::SignatureMatched
    );
    assert_eq!(
        ler_arquivo_resultado(&raiz_saida, &arquivo_pdf.caminho_relativo),
        pdf_minimo()
    );
}

/// Confirma que entrada ZIP com travessia não alcança o sistema de arquivos externo.
#[test]
fn rejeita_zip_slip_e_preserva_o_zip_raiz() {
    let temporario = tempfile::tempdir().unwrap();
    let diretorio_job = temporario.path().join("job");
    let raiz_saida = temporario.path().join("output");
    let pacote = criar_zip("../escape.txt", b"nao deve escapar");
    criar_manifesto(
        &diretorio_job,
        "unsafe.zip",
        "documents/unsafe.zip",
        &pacote,
    );

    let resultado = process_job_directory(&diretorio_job, &raiz_saida).unwrap();
    assert_eq!(resultado.estado, JobStatus::Partial);
    let expansao = resultado.anexos[0].expansao.as_ref().unwrap();
    assert!(
        expansao
            .avisos
            .iter()
            .any(|aviso| aviso.codigo == "ARCHIVE_PATH_REJECTED")
    );
    assert!(!temporario.path().join("escape.txt").exists());
    assert_eq!(
        fs::read(raiz_saida.join("documents/unsafe.zip")).unwrap(),
        pacote
    );
}

/// Confirma que Base64 inválido vai para quarentena sem publicar arquivo parcial.
#[test]
fn coloca_base64_invalido_em_quarentena() {
    let temporario = tempfile::tempdir().unwrap();
    let diretorio_job = temporario.path().join("job");
    let raiz_saida = temporario.path().join("output");
    fs::create_dir_all(&diretorio_job).unwrap();
    let manifesto = json!({
        "schema_version": "base64-file-converter.job.v1",
        "job_id": "job-invalid-base64",
        "correlation_id": "correlation-invalid-base64",
        "source_system": "sample-source",
        "attachments": [{
            "attachment_id": "attachment-invalid",
            "filename": "document.pdf",
            "content_base64": "%%%INVALID%%%",
            "output_relative_path": "documents/document.pdf"
        }]
    });
    fs::write(
        diretorio_job.join("job.json"),
        serde_json::to_vec_pretty(&manifesto).unwrap(),
    )
    .unwrap();

    let resultado = process_job_directory(&diretorio_job, &raiz_saida).unwrap();
    assert_eq!(resultado.estado, JobStatus::Failed);
    assert_eq!(resultado.anexos[0].estado, AttachmentStatus::Quarantined);
    assert!(!raiz_saida.join("documents/document.pdf").exists());
    assert!(diretorio_job.join("audit.jsonl").is_file());
}

/// Confirma que extensão desconhecida é reconstruída sem alterar nenhum byte.
#[test]
fn preserva_formato_desconhecido_byte_a_byte() {
    let temporario = tempfile::tempdir().unwrap();
    let diretorio_job = temporario.path().join("job");
    let raiz_saida = temporario.path().join("output");
    let conteudo = b"\x00\x01\x02\x03\xFE\xFFbinary";
    criar_manifesto(
        &diretorio_job,
        "sample.customblob",
        "documents/sample.customblob",
        conteudo,
    );

    let resultado = process_job_directory(&diretorio_job, &raiz_saida).unwrap();
    let decodificacao = resultado.anexos[0].decodificacao.as_ref().unwrap();
    assert_eq!(
        decodificacao.inspecao_midia.estado_assinatura,
        SignatureStatus::UnknownPreserved
    );
    assert_eq!(
        fs::read(raiz_saida.join("documents/sample.customblob")).unwrap(),
        conteudo
    );
}

/// Confirma que repetir o mesmo job reconhece o arquivo existente pelo hash.
#[test]
fn reprocessamento_e_idempotente() {
    let temporario = tempfile::tempdir().unwrap();
    let diretorio_job = temporario.path().join("job");
    let raiz_saida = temporario.path().join("output");
    criar_manifesto(
        &diretorio_job,
        "document.pdf",
        "documents/document.pdf",
        &pdf_minimo(),
    );

    let primeiro = process_job_directory(&diretorio_job, &raiz_saida).unwrap();
    let segundo = process_job_directory(&diretorio_job, &raiz_saida).unwrap();
    assert!(
        !primeiro.anexos[0]
            .decodificacao
            .as_ref()
            .unwrap()
            .arquivo_existente_idempotente
    );
    assert!(
        segundo.anexos[0]
            .decodificacao
            .as_ref()
            .unwrap()
            .arquivo_existente_idempotente
    );
}

/// Confirma que o resultado pode ser serializado sem vazar conteúdo Base64.
#[test]
fn resultado_nao_repete_content_base64() {
    let temporario = tempfile::tempdir().unwrap();
    let diretorio_job = temporario.path().join("job");
    let raiz_saida = temporario.path().join("output");
    criar_manifesto(
        &diretorio_job,
        "document.pdf",
        "documents/document.pdf",
        &pdf_minimo(),
    );
    let resultado: JobResult = process_job_directory(&diretorio_job, &raiz_saida).unwrap();
    let serializado: Value = serde_json::to_value(resultado).unwrap();
    let texto = serde_json::to_string(&serializado).unwrap();
    assert!(!texto.contains("content_base64"));
}
