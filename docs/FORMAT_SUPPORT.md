# Suporte de formatos

## Termos

- **Reconstruído:** os bytes Base64 foram materializados.
- **Validado:** existe evidência de assinatura, estrutura ou extensão.
- **Expandido:** arquivos filhos foram extraídos.
- **Preservado:** original mantido, mesmo sem parser.

## Matriz

| Família | Exemplos | Reconstrução | Validação | Expansão |
|---|---|---:|---:|---:|
| PDF | PDF, PDF/A | sim | cabeçalho + `%%EOF` | não aplicável |
| Imagens | JPEG, PNG, GIF, TIFF, BMP, WEBP, HEIC | sim | assinatura | não |
| Word | DOC, DOT, DOCX, DOCM, DOTX, DOTM | sim | OLE/ZIP | não |
| Excel | XLS, XLT, XLA, XLSX, XLSM, XLSB, XLTX, XLTM, XLAM | sim | OLE/ZIP | não |
| PowerPoint | PPT, POT, PPS, PPA, PPTX, PPTM, POTX, POTM, PPSX, PPSM, PPAM | sim | OLE/ZIP | não |
| OpenDocument | ODT, ODS, ODP | sim | ZIP | não |
| Texto | TXT, CSV, TSV, JSON, XML, YAML, HTML | sim | estrutura textual | não |
| E-mail MIME | EML, MHT, MHTML | sim | texto/MIME | sim |
| Outlook | MSG, OFT | sim | OLE | sim |
| Outlook stores | PST, OST, OLM, DBX | sim | parcial | não |
| Mailbox | MBOX, MBX | sim | textual/parcial | não |
| TNEF | `winmail.dat` | sim | assinatura | sim |
| Compactação | ZIP | sim | assinatura | sim |
| Compactação | 7z | sim | assinatura | sim |
| Compactação | RAR | sim | assinatura | sim, código nativo |
| Compactação | GZ, BZ2, XZ, TAR | sim | assinatura/extensão | ainda não |
| Outros | qualquer extensão | sim | assinatura inferida ou desconhecida | não |

## Office

DOCX, XLSX e PPTX usam ZIP internamente. Abri-los como arquivo compactado
produziria XMLs, relacionamentos e mídia que pertencem ao próprio documento,
não uma lista de anexos independentes. Por isso são preservados intactos.

## Outlook

MSG e OFT são mensagens individuais OLE. PST/OST são stores completos, com
índices, pastas e grande volume. O projeto diferencia essas capacidades para
evitar declarar uma store preservada como mensagem totalmente processada.

## Formatos desconhecidos

Um formato desconhecido recebe:

- MIME inferido por assinatura, quando possível;
- `application/octet-stream` caso contrário;
- `signature_status=unknown_preserved`;
- aviso auditável.

O arquivo não é executado nem descartado.
