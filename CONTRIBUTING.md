# Contribuição

## Antes de alterar

Abra uma issue descrevendo:

- problema;
- comportamento esperado;
- risco de compatibilidade;
- impacto em segurança e desempenho;
- formato ou corpus necessário.

Não envie dados reais, documentos privados ou credenciais.

## Padrões

- variáveis internas em português e nomes explícitos;
- JSON externo versionado e interoperável;
- `rustdoc` em toda função;
- nenhuma regra de negócio no núcleo;
- nenhuma escrita fora da raiz;
- nenhuma perda silenciosa;
- testes proporcionais ao risco;
- decisão de segurança documentada.

## Validação

Execute:

```powershell
.\scripts\run-qa.ps1
```

Pull requests devem explicar:

- o que muda;
- por que muda;
- como foi testado;
- riscos;
- compatibilidade;
- impacto no contrato JSON.
