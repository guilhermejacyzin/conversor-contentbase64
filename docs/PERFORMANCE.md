# Desempenho e memória

## Objetivo

O componente foi desenhado para processar arquivos grandes e muitos trabalhos
independentes sem fazer o consumo de memória crescer na mesma proporção do
tamanho do conteúdo. Rust elimina coleta de lixo e permite controlar buffers,
alocações, ownership e tempo de vida, mas desempenho sustentável ainda depende
de I/O, configuração e limites corretos.

## Caminho crítico

Para um arquivo de `n` bytes, a reconstrução executa:

1. leitura do texto Base64;
2. remoção incremental de espaços;
3. decodificação;
4. atualização do SHA-256;
5. captura limitada de cabeçalho e cauda;
6. gravação no temporário;
7. `flush` e, por padrão, `fsync`;
8. renomeação atômica.

Complexidade:

- tempo: `O(n)`;
- memória do núcleo: `O(1)` em relação ao tamanho do arquivo;
- disco: uma gravação do original;
- repetição idempotente: uma leitura adicional do destino existente para hash.

## Orçamento de memória

Os principais buffers do caminho de reconstrução são:

| Estrutura | Tamanho aproximado |
|---|---:|
| leitura Base64 normalizada | 64 KiB |
| decodificação e gravação | 256 KiB |
| prefixo para assinatura | 16 KiB |
| cauda para validação estrutural | 32 KiB |
| estado SHA-256 e estruturas auxiliares | pequeno e constante |

O processo também carrega código, bibliotecas, estruturas do sistema e
metadados. Por isso, o RSS observado é maior que a soma dos buffers. No smoke
benchmark de 50 MiB, o maior pico observado foi 10,82 MiB.

### Exceção: parsers que materializam o contêiner

O streaming do núcleo não transforma automaticamente toda dependência em
streaming. As bibliotecas atuais podem manter conteúdo proporcional ao arquivo:

- EML, MHT e MHTML são lidos para o parser MIME;
- MSG e OFT podem manter propriedades e payloads em memória;
- TNEF é lido integralmente pelo parser;
- a API usada para uma entrada RAR devolve seu conteúdo em memória;
- ZIP e 7z entregam leitores incrementais ao materializador.

Para esses formatos, limite `max_single_extracted_bytes`, reduza concorrência e
meça RSS com o corpus real. RAR não confiável deve ser processado sob limite de
memória do sistema operacional ou contêiner. Arquivos acima da capacidade
homologada devem ser preservados e encaminhados a um extrator separado.

### Exceção: `content_base64` inline

Quando o manifesto JSON contém `content_base64`, o parser JSON mantém essa
string na memória. Essa opção é funcional para integração simples, mas não é a
mais eficiente para arquivos grandes.

Para volume elevado, prefira:

1. gravar o campo Base64 em arquivo de staging e usar `base64_path`;
2. enviar por `stdin` quando houver um único conteúdo;
3. integrar a biblioteca com um `Read` de rede ou object storage;
4. evitar duplicar a string Base64 entre camadas e logs.

Essa decisão mantém a memória próxima do número de workers e dos buffers, não
da soma dos arquivos em trânsito.

## Modelo de concorrência

O spool cria um pool Rayon com quantidade fixa de threads. Jobs e anexos usam o
mesmo runtime; o work stealing distribui tarefas sem criar um pool por arquivo.

O valor ideal não é automaticamente o número de CPUs:

- Base64 + SHA-256 podem usar CPU;
- `fsync` e storage de rede aumentam espera;
- muitos arquivos pequenos elevam custo de metadados;
- descompactação pode consumir CPU e memória do codec;
- concorrência alta pode saturar disco e reduzir a vazão total.

Comece com 2 a 4 workers por volume de storage e aumente somente enquanto:

- bytes por segundo sobem;
- p95 e p99 permanecem aceitáveis;
- fila de disco não cresce continuamente;
- memória e espaço temporário mantêm margem;
- quarentena não aumenta.

## Fórmulas de capacidade

Volume binário diário:

```text
itens_por_dia × média_de_anexos_por_item × tamanho_médio_do_anexo
```

Volume Base64 aproximado:

```text
volume_binário × 4 / 3
```

Vazão sustentada mínima:

```text
volume_total / janela_disponível_em_segundos
```

Capacidade recomendada deve usar p95 e maior caso, não apenas média. Reserve
espaço simultâneo para fonte Base64, temporário, original, filhos extraídos,
auditoria e margem de recuperação.

## Escolhas deliberadas

### Buffer de 256 KiB

Reduz chamadas de I/O sem reservar dezenas de MiB por worker. Aumentar o buffer
pode ajudar em alguns storages, mas precisa de benchmark porque também multiplica
memória pela concorrência.

### SHA-256 na mesma passagem

Evita reler o temporário para provar integridade. O custo criptográfico existe,
mas substituí-lo por hash fraco reduziria a segurança de deduplicação e auditoria.

### `fsync` antes da publicação

Prioriza durabilidade. Desabilitar pode reduzir latência, mas cria risco de o
nome final existir antes de os bytes estarem persistidos fisicamente. A política
padrão mantém `fsync`.

### Publicação atômica

O temporário fica no mesmo diretório do destino. Assim, a renomeação não cruza
volumes e o consumidor nunca deve observar um arquivo parcialmente gravado.

### Expansão recursiva com orçamento

Cada árvore controla profundidade, quantidade, tamanho individual, total
expandido e taxa de compressão. Isso limita consumo acidental ou malicioso.

## Perfil de release

O perfil de produção usa:

- `opt-level = 3`;
- LTO fino;
- uma unidade de geração de código;
- verificações de overflow;
- `panic = abort`;
- remoção de símbolos.

O build release é mais lento, especialmente pela dependência RAR nativa. Esse é
um custo de compilação, não do processamento diário.

## Como validar eficiência

Antes de implantar:

1. use `cargo build --release --locked`;
2. execute corpus sem dados sensíveis;
3. separe reconstrução, e-mail e compactação;
4. teste concorrência 1, 2, 4, 8 e 16;
5. registre p50, p95, p99, MiB/s, CPU, RSS, IOPS e espaço;
6. repita em storage local e no storage real;
7. teste cold e warm cache;
8. confirme SHA-256 de todas as saídas;
9. force falhas de disco, limite e processo;
10. transforme o resultado medido em SLO operacional.

Veja [BENCHMARK.md](BENCHMARK.md) para o roteiro e a evidência local.
