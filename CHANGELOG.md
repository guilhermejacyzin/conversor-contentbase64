# Changelog

Todas as alterações relevantes deste projeto serão registradas neste arquivo.
O formato segue [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/) e
o versionamento segue [Semantic Versioning](https://semver.org/lang/pt-BR/).

## [Unreleased]

## [0.1.1] - 2026-07-30

### Corrigido

- criação do diretório `/app` antes do `chown` na imagem de runtime;
- atualização do checkout da CI para Node.js 24 com SHA imutável.

## [0.1.0] - 2026-07-30

### Adicionado

- Reconstrução Base64 em streaming com limite de tamanho.
- Publicação atômica, SHA-256, verificação de tamanho e idempotência.
- Validação de assinaturas e preservação auditada de formatos desconhecidos.
- Expansão recursiva de EML, MHT, MHTML, MSG, TNEF, ZIP, 7z e RAR.
- Proteções contra travessia, links simbólicos, bombas de compactação e recursão.
- Contrato genérico de jobs, auditoria JSONL e worker de spool.
- CLI, biblioteca Rust, produtor Python com retenção de falhas e documentação.
- Testes Rust e Python, integração ponta a ponta, Clippy e documentação Rust.
- CI em Windows e Linux, build de contêiner e política `cargo-deny`.
- Benchmark de referência, modelo de memória e roteiro de capacidade.

[Unreleased]: https://github.com/guilhermejacyzin/conversor-contentbase64/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/guilhermejacyzin/conversor-contentbase64/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/guilhermejacyzin/conversor-contentbase64/releases/tag/v0.1.0
