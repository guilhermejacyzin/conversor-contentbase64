"""Audita a arvore e o historico publico contra dados privados e segredos."""

from __future__ import annotations

import base64
import binascii
import os
import re
import subprocess
import sys
import unicodedata
from dataclasses import dataclass
from pathlib import Path


EXTENSOES_BASE64 = {".b64", ".base64"}


@dataclass(frozen=True)
class Ocorrencia:
    """Representa uma violacao sem repetir o conteudo potencialmente sensivel."""

    origem: str
    regra: str


def normalizar_texto(texto: str) -> str:
    """Remove variacoes de caixa e acento para impedir evasoes simples."""
    decomposto = unicodedata.normalize("NFKD", texto)
    sem_acentos = "".join(
        caractere
        for caractere in decomposto
        if not unicodedata.combining(caractere)
    )
    return sem_acentos.casefold()


def criar_expressao_termo(termo: str) -> str:
    """Aceita espaco, hifen ou sublinhado entre as partes de um nome reservado."""
    partes = re.split(r"[\s_-]+", normalizar_texto(termo))
    return r"[\s_-]+".join(re.escape(parte) for parte in partes)


def carregar_termos_reservados_externos() -> list[str]:
    """Carrega termos privados da memoria do processo, nunca de arquivo versionado."""
    conteudo = os.environ.get("AUDITORIA_TERMOS_RESERVADOS", "")
    return [
        termo.strip()
        for termo in conteudo.splitlines()
        if termo.strip()
    ]


def criar_padroes_proibidos(
    termos_reservados: list[str] | None = None,
) -> list[tuple[str, re.Pattern[str]]]:
    """Cria regras externas de nomes e regras genericas de caminho e segredo."""
    termos = (
        carregar_termos_reservados_externos()
        if termos_reservados is None
        else termos_reservados
    )
    padroes: list[tuple[str, re.Pattern[str]]] = []
    for indice, termo_reservado in enumerate(termos, start=1):
        termo = criar_expressao_termo(termo_reservado)
        padroes.append(
            (
                f"external-reserved-term-{indice:02d}",
                re.compile(
                    rf"(?<![a-z0-9]){termo}(?![a-z0-9])",
                    re.IGNORECASE,
                ),
            )
        )

    padroes.extend(
        [
            (
                "windows-user-path",
                re.compile(r"[a-z]:\\users\\[^\\\r\n]+\\", re.IGNORECASE),
            ),
            (
                "unix-home-path",
                re.compile(r"/ho" + r"me/(?!runner(?:/|$))[^/\r\n]+/", re.IGNORECASE),
            ),
            (
                "mac-user-path",
                re.compile(r"/us" + r"ers/[^/\r\n]+/", re.IGNORECASE),
            ),
            (
                "private-key-marker",
                re.compile(
                    r"begin\s+(?:rsa|openssh|ec|dsa)\s+private\s+key",
                    re.IGNORECASE,
                ),
            ),
            (
                "client-secret-assignment",
                re.compile(r"client[_-]?secret\s*[:=]\s*[^\s${}<]+", re.IGNORECASE),
            ),
            (
                "password-assignment",
                re.compile(r"password\s*[:=]\s*[^\s${}<]+", re.IGNORECASE),
            ),
            (
                "github-token",
                re.compile(r"(?:gh[pousr]_[a-z0-9]{20,}|github_pat_[a-z0-9_]{20,})", re.IGNORECASE),
            ),
            (
                "aws-access-key",
                re.compile(r"(?:akia|asia)[a-z0-9]{16}", re.IGNORECASE),
            ),
            (
                "slack-token",
                re.compile(r"xox[baprs]-[a-z0-9-]{10,}", re.IGNORECASE),
            ),
            (
                "jwt-token",
                re.compile(
                    r"eyj[a-z0-9_-]{8,}\.[a-z0-9_-]{8,}\.[a-z0-9_-]{8,}",
                    re.IGNORECASE,
                ),
            ),
            (
                "connection-string-secret",
                re.compile(
                    r"(?:accountkey|sharedaccesssignature)\s*=\s*[^\s;${}<]+",
                    re.IGNORECASE,
                ),
            ),
        ]
    )
    return padroes


def listar_arquivos_auditaveis(raiz: Path) -> list[Path]:
    """Lista arquivos controlados ou novos, excluindo caches ignorados pelo Git."""
    resultado = subprocess.run(
        [
            "git",
            "-C",
            str(raiz),
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
        check=True,
        capture_output=True,
    )
    return [
        raiz / caminho.decode("utf-8")
        for caminho in resultado.stdout.split(b"\0")
        if caminho
    ]


def auditar_texto(
    texto: str,
    origem: str,
    padroes: list[tuple[str, re.Pattern[str]]],
) -> list[Ocorrencia]:
    """Aplica todas as regras ao texto normalizado e retorna apenas identificadores."""
    texto_normalizado = normalizar_texto(texto)
    return [
        Ocorrencia(origem=origem, regra=identificador)
        for identificador, padrao in padroes
        if padrao.search(texto_normalizado)
    ]


def extrair_textos_de_listas_numericas(texto: str) -> list[str]:
    """Decodifica listas de bytes ASCII para detectar ofuscacao textual simples."""
    padrao_lista = re.compile(r"\[(?:\s*\d{1,3}\s*,){2,}\s*\d{1,3}\s*,?\s*\]")
    textos: list[str] = []
    for correspondencia in padrao_lista.finditer(texto):
        numeros = [
            int(valor)
            for valor in re.findall(r"\d{1,3}", correspondencia.group(0))
        ]
        if any(valor > 255 for valor in numeros):
            continue
        try:
            decodificado = bytes(numeros).decode("ascii")
        except UnicodeDecodeError:
            continue
        if decodificado.isprintable():
            textos.append(decodificado)
    return textos


def remover_prefixo_data_uri(texto: str) -> str:
    """Remove um cabecalho Data URI para validar somente a carga Base64."""
    candidato = texto.strip()
    if candidato.casefold().startswith("data:") and "," in candidato:
        return candidato.split(",", maxsplit=1)[1]
    return candidato


def auditar_base64(
    dados: bytes,
    origem: str,
    padroes: list[tuple[str, re.Pattern[str]]],
) -> list[Ocorrencia]:
    """Decodifica uma fixture Base64 e impede que ela esconda conteudo binario."""
    try:
        texto = dados.decode("utf-8")
        carga = re.sub(r"\s+", "", remover_prefixo_data_uri(texto))
        decodificado = base64.b64decode(carga, validate=True)
    except (UnicodeDecodeError, binascii.Error, ValueError):
        return [Ocorrencia(origem=origem, regra="base64-fixture-invalid")]

    try:
        texto_decodificado = decodificado.decode("utf-8")
    except UnicodeDecodeError:
        return [Ocorrencia(origem=origem, regra="base64-binary-fixture-not-allowed")]

    return auditar_texto(
        texto_decodificado,
        f"{origem}:decoded-base64",
        padroes,
    )


def auditar_bytes(
    dados: bytes,
    origem: str,
    sufixo: str,
    padroes: list[tuple[str, re.Pattern[str]]],
) -> list[Ocorrencia]:
    """Inspeciona texto, rejeita binarios versionados e abre cargas Base64."""
    ocorrencias: list[Ocorrencia] = []
    try:
        texto = dados.decode("utf-8")
    except UnicodeDecodeError:
        texto = dados.decode("latin-1", errors="ignore")
        ocorrencias.append(
            Ocorrencia(origem=origem, regra="binary-artifact-not-allowed")
        )

    if b"\0" in dados:
        ocorrencias.append(
            Ocorrencia(origem=origem, regra="binary-artifact-not-allowed")
        )

    ocorrencias.extend(auditar_texto(texto, origem, padroes))
    for indice, texto_extraido in enumerate(
        extrair_textos_de_listas_numericas(texto),
        start=1,
    ):
        ocorrencias.extend(
            auditar_texto(
                texto_extraido,
                f"{origem}:numeric-byte-list-{indice}",
                padroes,
            )
        )
    if sufixo.casefold() in EXTENSOES_BASE64:
        ocorrencias.extend(auditar_base64(dados, origem, padroes))
    return ocorrencias


def auditar_arvore(
    raiz: Path,
    padroes: list[tuple[str, re.Pattern[str]]],
) -> list[Ocorrencia]:
    """Audita nomes, caminhos e bytes de cada arquivo atualmente versionado."""
    ocorrencias: list[Ocorrencia] = []
    for caminho in listar_arquivos_auditaveis(raiz):
        relativo = caminho.relative_to(raiz).as_posix()
        ocorrencias.extend(auditar_texto(relativo, relativo, padroes))
        ocorrencias.extend(
            auditar_bytes(caminho.read_bytes(), relativo, caminho.suffix, padroes)
        )
    return ocorrencias


def listar_blobs_historicos(raiz: Path) -> list[tuple[str, str]]:
    """Resolve os blobs alcancaveis por branches e tags com seus caminhos conhecidos."""
    resultado = subprocess.run(
        ["git", "-C", str(raiz), "rev-list", "--objects", "--all"],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    blobs: list[tuple[str, str]] = []
    vistos: set[str] = set()
    for linha in resultado.stdout.splitlines():
        objeto, _, caminho = linha.partition(" ")
        if objeto in vistos:
            continue
        tipo = subprocess.run(
            ["git", "-C", str(raiz), "cat-file", "-t", objeto],
            check=True,
            capture_output=True,
            text=True,
            encoding="ascii",
        ).stdout.strip()
        if tipo == "blob":
            vistos.add(objeto)
            blobs.append((objeto, caminho))
    return blobs


def ler_blob(raiz: Path, objeto: str) -> bytes:
    """Le um blob pelo protocolo do Git sem materializa-lo no diretorio de trabalho."""
    return subprocess.run(
        ["git", "-C", str(raiz), "cat-file", "blob", objeto],
        check=True,
        capture_output=True,
    ).stdout


def auditar_historico(
    raiz: Path,
    padroes: list[tuple[str, re.Pattern[str]]],
) -> list[Ocorrencia]:
    """Audita todos os blobs, caminhos e metadados Git ainda alcancaveis."""
    ocorrencias: list[Ocorrencia] = []
    for objeto, caminho in listar_blobs_historicos(raiz):
        origem = f"git:{objeto}:{caminho or '<sem-caminho>'}"
        ocorrencias.extend(auditar_texto(caminho, origem, padroes))
        ocorrencias.extend(
            auditar_bytes(
                ler_blob(raiz, objeto),
                origem,
                Path(caminho).suffix,
                padroes,
            )
        )

    metadados = subprocess.run(
        [
            "git",
            "-C",
            str(raiz),
            "log",
            "--all",
            "--format=%H%n%an%n%ae%n%B",
        ],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    ).stdout
    ocorrencias.extend(auditar_texto(metadados, "git:metadata", padroes))
    return ocorrencias


def remover_duplicidades(ocorrencias: list[Ocorrencia]) -> list[Ocorrencia]:
    """Consolida a mesma regra na mesma origem para manter o relatorio acionavel."""
    unicas = {(ocorrencia.origem, ocorrencia.regra): ocorrencia for ocorrencia in ocorrencias}
    return [unicas[chave] for chave in sorted(unicas)]


def principal() -> int:
    """Executa o gate completo e devolve codigo diferente de zero em violacao."""
    raiz = Path(__file__).resolve().parent.parent
    padroes = criar_padroes_proibidos()
    ocorrencias = remover_duplicidades(
        auditar_arvore(raiz, padroes) + auditar_historico(raiz, padroes)
    )
    if ocorrencias:
        print("Auditoria publica falhou:", file=sys.stderr)
        for ocorrencia in ocorrencias:
            print(
                f"- {ocorrencia.origem}: {ocorrencia.regra}",
                file=sys.stderr,
            )
        return 1

    print("Auditoria publica concluida sem ocorrencias na arvore ou no historico.")
    return 0


if __name__ == "__main__":
    raise SystemExit(principal())
