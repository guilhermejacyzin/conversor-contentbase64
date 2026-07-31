# Segurança

## Modelo de ameaça

Todo campo do manifesto e todo byte do arquivo são considerados não confiáveis.

Ameaças consideradas:

- Base64 inválido ou truncado;
- nome com travessia de diretório;
- junction ou link simbólico para fora da raiz;
- extensão falsa;
- arquivo parcial após falha;
- sobrescrita de evidência;
- ZIP Slip;
- links em arquivo compactado;
- bomba de compactação;
- recursão cíclica ou excessiva;
- milhares de anexos pequenos;
- um único anexo enorme;
- arquivo criptografado;
- parser nativo vulnerável;
- vazamento de conteúdo em log;
- corrida entre workers;
- reinício no meio da publicação.

## Controles

### Entrada

- versão de contrato obrigatória;
- identificadores com conjunto de caracteres limitado;
- uma única fonte por anexo;
- caminhos relativos validados;
- fonte canonicalizada dentro do job;
- destinos duplicados rejeitados.

### Reconstrução

- limite antes de publicar;
- Base64 em streaming;
- SHA-256;
- tamanho esperado opcional;
- hash esperado opcional;
- assinatura/MIME;
- temporário exclusivo;
- `flush` e `fsync`;
- renomeação;
- conflito sem sobrescrita.

### Expansão

- profundidade máxima;
- quantidade máxima;
- tamanho individual;
- total expandido;
- taxa de compactação;
- nomes sanitizados;
- caminhos de arquivo não reutilizados;
- links ZIP rejeitados;
- entradas criptografadas sinalizadas;
- original sempre preservado.

### Auditoria

- eventos sem Base64;
- erros sem quebra de linha e limitados a 2.000 caracteres;
- sequência por tentativa;
- `flush` por evento;
- `fsync` opcional;
- resultado estruturado.

## RAR

O extrator RAR utiliza o código nativo incorporado por `unrar`. Isso cria uma
fronteira diferente dos parsers Rust:

- maior superfície de memória;
- licença própria do componente UnRAR;
- build C++;
- dependência de bibliotecas do sistema;
- necessidade maior de sandbox.

Recomendação para dados não confiáveis:

- contêiner sem privilégios;
- usuário não root;
- filesystem somente nos diretórios necessários;
- sem socket Docker;
- sem credenciais na imagem;
- sem rede de saída;
- limites de CPU, memória, PIDs e disco;
- atualização e revalidação periódicas da dependência.

## Malware

Validar assinatura não equivale a antimalware. O componente não executa bytes,
mas consumidores posteriores podem ser vulneráveis. Integre antivírus/CDR antes
de abrir arquivos em aplicações de usuário ou renderizadores.

## Criptografia

Arquivos criptografados são preservados e sinalizados. O projeto não recebe
senhas no manifesto para evitar segredos em spool e auditoria. Um serviço de
descriptografia deve ser separado, controlado e auditado.

## Segredos

O conversor não precisa de credenciais. Adaptadores devem obter segredos por
secret manager e enviar apenas arquivos/metadados necessários ao núcleo.

## Riscos residuais

- EML, MSG, TNEF e entradas RAR podem exigir memória proporcional ao conteúdo
  por característica dos parsers; aplique limites de memória externos.
- um processo local com permissão de escrita na árvore de saída pode tentar
  trocar diretórios por links ou junctions entre validação e gravação; dedique a
  raiz ao worker, restrinja ACLs e use contêiner ou usuário exclusivo.
- `fsync` depende da semântica real do filesystem e do storage subjacente.
- assinatura e MIME não substituem antivírus, CDR ou sandbox.
- hash sem valor confiável fornecido pela origem prova consistência do pipeline,
  não a correção documental da origem.

## Relato de vulnerabilidade

Não publique detalhes exploráveis em issue pública. Use o canal privado de
segurança configurado no repositório ou contate o mantenedor.
