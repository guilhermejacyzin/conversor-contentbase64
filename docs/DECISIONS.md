# Decisões e críticas

## Rust

**Escolha:** memória segura, binário standalone e alto controle de I/O.

**Crítica:** Rust não elimina bugs de lógica, ZIP bombs ou vulnerabilidades em
dependências nativas. Build e onboarding são mais exigentes que um script curto.

## Streaming

**Escolha:** decodificar e hashear na mesma passagem.

**Crítica:** validações que exigem o arquivo inteiro continuam limitadas a
prefixo/cauda ou parsers posteriores. MSG e EML podem exigir memória proporcional
ao arquivo por característica das bibliotecas usadas.

## SHA-256

**Escolha:** integridade, idempotência e deduplicação.

**Crítica:** sem hash fornecido pela origem, SHA-256 prova consistência do que foi
recebido e publicado, mas não prova que a origem enviou o documento correto.

## Preservar desconhecidos

**Escolha:** não perder anexos por catálogo incompleto.

**Crítica:** preservado não significa validado nem seguro. Consumidores precisam
respeitar `signature_status`.

## Atomicidade

**Escolha:** temporário no mesmo diretório e renomeação.

**Crítica:** a garantia depende do filesystem. Shares de rede e object storage
podem ter semântica diferente e devem ser homologados.

## `fsync`

**Escolha:** habilitado para arquivos; configurável por evento de auditoria.

**Crítica:** durabilidade maior custa IOPS. O modo correto depende do RPO.

## Spool

**Escolha:** integração simples e auditável para filesystem.

**Crítica:** não é fila distribuída. Não oferece lease, visibilidade temporizada,
dead letter global ou ordenação. Em múltiplos hosts, use uma fila real.

## RAR

**Escolha:** cobertura funcional por `unrar`.

**Crítica:** código C/C++, licença própria, link mais complexo e superfície maior.
Mantido em função isolada para futura substituição ou processo sandboxado.

## PST/OST

**Escolha:** preservar e marcar como sem extrator.

**Crítica:** reduz cobertura imediata, mas evita falsa garantia. Um parser futuro
deve ter corpus, limites, benchmark e testes de recuperação antes de ativação.

## Concorrência

**Escolha:** Rayon para jobs e anexos.

**Crítica:** paralelismo não cria largura de banda. O default é conservador, mas
a configuração final precisa de benchmark no storage real.
