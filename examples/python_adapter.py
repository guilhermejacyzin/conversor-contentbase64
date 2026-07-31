"""Adaptador genérico de uma resposta JSON para o spool do conversor.

Este exemplo não faz chamadas HTTP e não contém credenciais. Ele demonstra a
fronteira recomendada: Python organiza metadados e Base64; Rust reconstrói,
valida, expande e audita os arquivos.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import uuid
from pathlib import Path
from typing import Any


PADRAO_IDENTIFICADOR = re.compile(r"^[A-Za-z0-9_.:@-]{1,128}$")


def validar_identificador(nome_campo: str, valor: str) -> str:
    """Valida um identificador antes de utilizá-lo em metadados ou caminhos."""
    if not PADRAO_IDENTIFICADOR.fullmatch(valor):
        raise ValueError(
            f"{nome_campo} deve conter de 1 a 128 caracteres "
            "alfanuméricos ou -_.:@"
        )
    return valor


def sanitizar_nome_arquivo(nome_arquivo: str, indice: int) -> str:
    """Remove diretórios e caracteres perigosos mantendo uma extensão útil."""
    nome_base = nome_arquivo.replace("\\", "/").rsplit("/", maxsplit=1)[-1].strip()
    nome_limpo = re.sub(r'[<>:"/\\|?*\x00-\x1f]', "_", nome_base).rstrip(" .")
    if not nome_limpo:
        nome_limpo = f"attachment-{indice:04}.bin"
    return nome_limpo[:180]


def gravar_texto_duravel(caminho: Path, conteudo: str) -> None:
    """Grava texto UTF-8, descarrega o buffer e solicita sincronização ao sistema."""
    caminho.parent.mkdir(parents=True, exist_ok=True)
    with caminho.open("x", encoding="utf-8", newline="\n") as arquivo:
        arquivo.write(conteudo)
        arquivo.flush()
        os.fsync(arquivo.fileno())


def gravar_json_duravel(caminho: Path, valor: dict[str, Any]) -> None:
    """Serializa JSON de forma legível usando a mesma garantia de gravação durável."""
    gravar_texto_duravel(
        caminho,
        json.dumps(valor, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
    )


def construir_manifesto(
    resposta: dict[str, Any],
    identificador_job: str,
    identificador_correlacao: str,
    sistema_origem: str,
    fluxo: str | None,
    identificador_operacao: str | None,
) -> tuple[dict[str, Any], list[tuple[str, str]]]:
    """Converte a lista genérica `attachments` em manifesto e payloads separados."""
    anexos_origem = resposta.get("attachments")
    if not isinstance(anexos_origem, list) or not anexos_origem:
        raise ValueError("a resposta deve conter uma lista não vazia em attachments")

    anexos_manifesto: list[dict[str, Any]] = []
    payloads: list[tuple[str, str]] = []
    for indice, anexo_origem in enumerate(anexos_origem, start=1):
        if not isinstance(anexo_origem, dict):
            raise ValueError(f"attachments[{indice - 1}] deve ser um objeto")
        nome_recebido = anexo_origem.get("filename")
        conteudo_base64 = anexo_origem.get("contentBase64")
        if not isinstance(nome_recebido, str) or not nome_recebido.strip():
            raise ValueError(f"attachments[{indice - 1}].filename é obrigatório")
        if not isinstance(conteudo_base64, str):
            raise ValueError(f"attachments[{indice - 1}].contentBase64 é obrigatório")

        identificador_anexo = f"attachment-{indice:04}"
        nome_seguro = sanitizar_nome_arquivo(nome_recebido, indice)
        caminho_payload = f"payloads/{identificador_anexo}.b64"
        caminho_saida = (
            f"records/{identificador_correlacao}/"
            f"{identificador_anexo}/{nome_seguro}"
        )
        anexos_manifesto.append(
            {
                "attachment_id": identificador_anexo,
                "filename": nome_recebido,
                "base64_path": caminho_payload,
                "output_relative_path": caminho_saida,
                "expand_containers": True,
            }
        )
        payloads.append((caminho_payload, conteudo_base64))

    manifesto: dict[str, Any] = {
        "schema_version": "base64-file-converter.job.v1",
        "job_id": identificador_job,
        "correlation_id": identificador_correlacao,
        "source_system": sistema_origem,
        "business_keys": {"record_id": identificador_correlacao},
        "attachments": anexos_manifesto,
    }
    if fluxo:
        manifesto["flow"] = fluxo
    if identificador_operacao:
        manifesto["operation_id"] = identificador_operacao
    return manifesto, payloads


def preservar_publicacao_falha(
    raiz_spool: Path,
    diretorio_temporario: Path,
) -> Path:
    """Move o staging incompleto para retenção forense sem excluir seus bytes."""
    diretorio_falhas_produtor = raiz_spool.resolve() / "producer-failed"
    diretorio_falhas_produtor.mkdir(parents=True, exist_ok=True)
    caminho_preservado = diretorio_falhas_produtor / diretorio_temporario.name
    diretorio_temporario.rename(caminho_preservado)
    return caminho_preservado


def publicar_job(
    raiz_spool: Path,
    manifesto: dict[str, Any],
    payloads: list[tuple[str, str]],
) -> Path:
    """Publica um job completo por staging, rename e marcador READY final."""
    diretorio_entrada = raiz_spool.resolve() / "inbox"
    diretorio_entrada.mkdir(parents=True, exist_ok=True)
    identificador_job = validar_identificador("job_id", str(manifesto["job_id"]))
    diretorio_final = diretorio_entrada / identificador_job
    if diretorio_final.exists():
        raise FileExistsError(f"job já existe: {diretorio_final}")

    diretorio_temporario = diretorio_entrada / (
        f".{identificador_job}.{uuid.uuid4().hex}.tmp"
    )
    diretorio_temporario.mkdir()
    try:
        for caminho_relativo, conteudo_base64 in payloads:
            gravar_texto_duravel(
                diretorio_temporario / Path(caminho_relativo),
                conteudo_base64,
            )
        gravar_json_duravel(diretorio_temporario / "job.json", manifesto)
        diretorio_temporario.rename(diretorio_final)
        gravar_texto_duravel(diretorio_final / "READY", "")
        return diretorio_final
    except Exception as erro_publicacao:
        if diretorio_temporario.exists():
            try:
                caminho_preservado = preservar_publicacao_falha(
                    raiz_spool,
                    diretorio_temporario,
                )
            except Exception as erro_preservacao:
                raise RuntimeError(
                    "publicação falhou; staging permaneceu no local temporário, "
                    f"mas a movimentação para retenção também falhou: {erro_preservacao}"
                ) from erro_publicacao
            raise RuntimeError(
                f"publicação falhou; staging preservado em {caminho_preservado}"
            ) from erro_publicacao
        raise


def analisar_argumentos() -> argparse.Namespace:
    """Define parâmetros sem caminhos ou sistemas fixos no código."""
    analisador = argparse.ArgumentParser(
        description="Publica uma resposta JSON no spool do conversor Base64"
    )
    analisador.add_argument("--input-json", required=True, type=Path)
    analisador.add_argument("--spool-root", required=True, type=Path)
    analisador.add_argument("--job-id", required=True)
    analisador.add_argument("--correlation-id", required=True)
    analisador.add_argument("--source-system", required=True)
    analisador.add_argument("--flow")
    analisador.add_argument("--operation-id")
    return analisador.parse_args()


def main() -> int:
    """Lê a resposta, monta o contrato e publica o job sem decodificar anexos."""
    argumentos = analisar_argumentos()
    identificador_job = validar_identificador("job_id", argumentos.job_id)
    identificador_correlacao = validar_identificador(
        "correlation_id", argumentos.correlation_id
    )
    with argumentos.input_json.open(encoding="utf-8") as arquivo:
        resposta = json.load(arquivo)
    if not isinstance(resposta, dict):
        raise ValueError("o JSON de entrada deve ser um objeto")

    manifesto, payloads = construir_manifesto(
        resposta=resposta,
        identificador_job=identificador_job,
        identificador_correlacao=identificador_correlacao,
        sistema_origem=argumentos.source_system,
        fluxo=argumentos.flow,
        identificador_operacao=argumentos.operation_id,
    )
    caminho = publicar_job(argumentos.spool_root, manifesto, payloads)
    print(json.dumps({"status": "published", "job_directory": str(caminho)}))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as erro:
        print(json.dumps({"status": "failed", "error": str(erro)}), file=sys.stderr)
        raise SystemExit(1) from erro
