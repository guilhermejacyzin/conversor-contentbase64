# Qualidade e testes

## Gates obrigatórios

1. `cargo fmt --all --check`
2. `cargo check --all-targets --locked`
3. `cargo clippy --all-targets --locked -- -D warnings`
4. `cargo test --all-targets --locked`
5. `cargo doc --no-deps --locked`
6. `cargo build --release --locked`
7. `python -m unittest discover -s tests_python -v`
8. `python scripts/audit_public_repo.py` na árvore e no histórico Git completo
9. `cargo deny check`
10. teste do binário release

## Estratégia

### Unidade

- normalização Base64 e Data URI;
- idempotência;
- assinatura e MIME;
- nomes e caminhos;
- estado de job;
- opções de spool.

### Integração

- EML -> EML -> PDF;
- ZIP com `../`;
- Base64 inválido;
- formato desconhecido;
- repetição idempotente;
- resultado sem `content_base64`.

### Produtor Python

- `READY` publicado por último;
- staging preservado em falha;
- nenhum payload apagado pelo caminho de exceção.

### Segurança pendente para evolução

- fuzzing contínuo de manifestos e Base64;
- corpus de MSG malformado;
- corpus RAR/7z criptografado e corrompido;
- hardlinks e tipos especiais TAR quando houver extrator;
- testes de exaustão de descritores;
- teste de queda forçada entre `fsync` e `rename`.

## Critérios de aceite

- nenhum `.part` após falha tratada;
- hash igual ao conteúdo esperado;
- nenhuma escrita fora da raiz;
- destino divergente preservado;
- fonte do job mantida no estado final;
- aviso estruturado para capacidade parcial;
- zero warnings do Clippy;
- documentação pública compilável;
- zero referência a cliente, contrato privado, caminho pessoal ou credencial;
- zero artefato binário versionado sem revisão e autorização explícitas;
- zero conteúdo reservado oculto em fixtures Base64.

Termos privados adicionais devem ser fornecidos apenas em memória pela variável
`AUDITORIA_TERMOS_RESERVADOS`, com um termo por linha. Eles não podem ser
gravados no repositório, em configuração pública ou em logs da CI.
