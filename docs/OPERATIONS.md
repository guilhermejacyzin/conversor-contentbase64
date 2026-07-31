# Operação

## Pré-implantação

- dimensionar bytes por dia, p95 e maior arquivo;
- medir expansão máxima de arquivos compactados reais;
- definir retenção da fonte Base64;
- definir destino de originais e filhos;
- definir owner de quarentena;
- configurar capacidade e alerta de disco;
- testar corpus representativo;
- registrar versão do binário e `Cargo.lock`.

## Sizing

Espaço temporário de pior caso por worker:

```text
originais em processamento
+ filhos extraídos
+ fontes Base64 retidas
+ margem operacional
```

Base64 ocupa aproximadamente 4/3 do binário, sem contar JSON. Não use a média
como único dimensionamento; use maior caso e percentis altos.

## Concorrência

Comece com:

- storage local SSD: 4 jobs;
- compartilhamento de rede: 2 jobs;
- muitos arquivos pequenos: aumente gradualmente;
- arquivos compactados CPU-intensivos: limite pela CPU.

Meça antes de elevar. Sinais de excesso:

- throughput total cai;
- latência p99 sobe;
- fila de disco aumenta;
- memória cresce;
- timeouts no storage.

## Implantação

### Serviço

Execute:

```text
base64-file-converter spool --raiz-spool <spool> --raiz-saida <output> --observar
```

O gerenciador de serviço deve:

- reiniciar em falha não zero;
- enviar `Ctrl+C`/SIGINT no encerramento;
- limitar reinícios rápidos;
- capturar stdout JSON e stderr;
- usar conta sem privilégios.

### Container

Monte:

- spool com leitura e escrita;
- output com leitura e escrita;
- configuração somente leitura.

Não monte diretórios sensíveis do host.

## Retomada

Itens em `processing` podem indicar interrupção. Antes de usar
`--recuperar-processamento`:

1. confirme que não existe outro worker ativo;
2. preserve snapshot da pasta;
3. verifique `audit.jsonl`;
4. execute uma varredura controlada;
5. confirme idempotência no resultado.

## Quarentena

Cada item deve receber:

- código de aviso/erro;
- owner;
- prazo;
- decisão de reprocessar, corrigir origem ou aceitar capacidade limitada;
- evidência do resultado.

Nunca mova um item de quarentena para concluído apenas porque o arquivo abre.
Verifique hash, linhagem e completude da expansão.

Falhas ocorridas no adaptador Python antes da publicação de `READY` são movidas
para `producer-failed`. Esse diretório não é consumido pelo worker. O staging
permanece intacto para diagnóstico e reenvio; a rotina não exclui payloads.

## Monitoramento

Métricas mínimas:

- jobs recebidos/concluídos/parciais/quarentena/falhos;
- anexos e bytes;
- duração p50/p95/p99;
- throughput;
- profundidade máxima observada;
- códigos de aviso;
- idade do job mais antigo;
- uso livre de disco;
- taxa de conflito;
- taxa de formato desconhecido.

## Backup e retenção

Defina separadamente:

- fonte Base64;
- original;
- filhos;
- auditoria;
- metadados.

Hashes não substituem backup. Backup não substitui política de retenção.
