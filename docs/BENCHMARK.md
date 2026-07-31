# Benchmark

## Objetivo

Medir throughput e latência no ambiente de destino. Números de outro hardware
não devem ser usados como SLA.

## Cenários

1. arquivo pequeno: 100 KiB;
2. arquivo médio: 10 MiB;
3. arquivo grande: 200 MiB;
4. PDF grande sem expansão;
5. ZIP com muitos arquivos pequenos;
6. ZIP com poucos arquivos grandes;
7. EML com anexos;
8. EML aninhado;
9. concorrência 1, 2, 4, 8 e 16.

## Métricas

- duração p50/p95/p99;
- MiB/s;
- CPU;
- memória de pico;
- IOPS;
- fila de disco;
- bytes temporários;
- tempo de `fsync`;
- tempo de expansão;
- taxa de quarentena.

## Método

- usar build `--release --locked`;
- aquecer cache separadamente;
- executar no mínimo 30 amostras por cenário;
- medir cold e warm cache;
- registrar tipo de storage;
- manter corpus e hashes;
- não usar dados sensíveis;
- repetir após mudança de codec ou dependência.

## Interpretação

Base64 aumenta o volume transportado em aproximadamente 33%. A reconstrução
também calcula SHA-256 e grava o arquivo. Compare o throughput total do pipeline,
não apenas uma função isolada em memória.

## Evidência local de referência

Esta medição é um *smoke benchmark* de engenharia, útil para comprovar
integridade, baixo consumo de memória e ordem de grandeza. Ela não substitui o
benchmark formal de 30 ou mais amostras no ambiente de destino.

Data da medição: 2026-07-30.

Ambiente:

- Intel Core i5-8250U, 4 núcleos e 8 processadores lógicos;
- 16 GiB de memória física;
- volume local NTFS;
- executável compilado com `cargo build --release --locked`;
- arquivo binário sintético de 50 MiB;
- entrada Base64 contínua de 69.905.068 bytes;
- validação de tamanho e SHA-256 habilitada;
- sincronização durável e publicação atômica habilitadas.

| Amostra | Duração | Throughput | Pico de memória | SHA-256 |
|---:|---:|---:|---:|---|
| 1 | 11,5219 s | 4,34 MiB/s | 10,81 MiB | confirmado |
| 2 | 8,0960 s | 6,18 MiB/s | 10,68 MiB | confirmado |
| 3 | 2,6609 s | 18,79 MiB/s | 10,82 MiB | confirmado |
| **Média** | **7,4263 s** | **9,77 MiB/s** | **10,77 MiB** | **confirmado** |

Resumo:

- menor tempo: 2,6609 s;
- maior tempo: 11,5219 s;
- média: 7,4263 s;
- maior pico de memória observado: 10,82 MiB;
- três saídas com o mesmo SHA-256 do binário de origem.

A dispersão entre amostras é material. Cache do sistema operacional, pressão de
I/O, sincronização física e inspeção antimalware podem alterar a duração. Para
dimensionar produção, execute o corpus real no mesmo storage, rede, política de
segurança e nível de concorrência que serão usados na operação.
