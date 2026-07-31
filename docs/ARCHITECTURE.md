# Arquitetura

## Objetivo

Fornecer uma fronteira confiável entre sistemas que transportam binários como
Base64 e sistemas que precisam dos arquivos originais, sem incorporar regra de
negócio, caminho fixo ou dependência de um acionador específico.

## Princípios

1. **Preservação antes de interpretação:** formato desconhecido não é descartado.
2. **Publicação após validação:** temporários nunca são apresentados como resultado.
3. **Idempotência verificável:** repetição depende de tamanho e SHA-256.
4. **Falha isolada:** anexos independentes continuam quando a política permite.
5. **Recursão limitada:** toda árvore possui orçamento finito.
6. **Contrato estável:** integrações dependem de JSON versionado, não de internals.
7. **Conteúdo fora do banco operacional:** resultados referenciam arquivos por URI.

## Componentes

### Decoder

`decoder.rs` recebe qualquer `Read`, remove espaços, reconhece Data URI opcional,
decodifica em blocos, calcula SHA-256, captura prefixo/cauda, sincroniza e publica.

Complexidade:

- tempo: `O(n)`;
- memória: `O(buffer + prefixo + cauda)`;
- I/O: uma gravação do arquivo e, em repetição, uma leitura adicional do destino.

### Media

`media.rs` combina assinatura binária, extensão e MIME. A classificação registra
o nível de confiança:

- `signature_matched`;
- `structural_match`;
- `extension_only`;
- `unknown_preserved`;
- `validation_disabled`.

Uma divergência forte de formato bloqueia a publicação do arquivo raiz. Para um
filho extraído, os bytes permanecem preservados e a árvore vira parcial.

### Expansion

`expansion.rs` seleciona extratores por formato. Cada filho:

1. recebe nome sanitizado e identificador;
2. é gravado em streaming quando o parser permite;
3. recebe SHA-256 e inspeção de mídia;
4. é registrado com `parent_file_id`;
5. volta ao seletor de contêiner.

Pacotes Office não são expandidos como ZIP. Seus XMLs internos fazem parte do
documento e não são anexos independentes.

### Job

`job.rs` valida manifesto, caminhos, duplicidades e fontes. Anexos podem ser
processados em paralelo. Eventos são persistidos em JSONL sem conteúdo Base64.

### Spool

`spool.rs` implementa um adaptador simples:

```text
inbox -> processing -> completed | quarantine | failed
```

A reivindicação por renomeação é atômica quando origem e destino estão no mesmo
volume. O spool não substitui uma fila distribuída; é uma opção para worker local
ou integração baseada em filesystem.

## Modelo de concorrência

- jobs: pool Rayon configurável;
- anexos do mesmo job: `par_iter`;
- auditoria: `Mutex<File>` por job para preservar linhas completas;
- arquivos: destinos exclusivos e publicação atômica;
- orçamento de expansão: local a cada anexo raiz, sem estado global.

Concorrência de job e anexo usa o mesmo runtime Rayon. O work stealing evita
criar uma quantidade de threads proporcional a `jobs x anexos`.

## Modelo de dados

### Conteúdo

Armazenado em filesystem ou object storage:

- original;
- filhos;
- manifestação de entrada;
- auditoria;
- resultado.

### Catálogo

Armazenado em banco:

- identidade;
- correlação;
- linhagem;
- URI;
- hash;
- tamanho;
- MIME;
- capacidade;
- estado;
- timestamps.

## Evolução

Novos acionadores implementam somente a produção do contrato. Novos formatos
implementam um extrator que utiliza `materializar_e_processar`, herdando hash,
limites, nomes seguros, atomicidade, auditoria e recursão.
