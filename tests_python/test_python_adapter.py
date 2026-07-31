"""Testes do adaptador produtor, com foco em atomicidade e não perda."""

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path
from types import ModuleType


def carregar_adaptador() -> ModuleType:
    """Carrega o exemplo Python diretamente do arquivo versionado."""
    caminho = (
        Path(__file__).resolve().parent.parent / "examples" / "python_adapter.py"
    )
    especificacao = importlib.util.spec_from_file_location(
        "python_adapter",
        caminho,
    )
    if especificacao is None or especificacao.loader is None:
        raise RuntimeError("não foi possível carregar o adaptador Python")
    modulo = importlib.util.module_from_spec(especificacao)
    especificacao.loader.exec_module(modulo)
    return modulo


def criar_manifesto() -> dict[str, object]:
    """Cria o menor manifesto suficiente para testar a publicação do produtor."""
    return {
        "schema_version": "base64-file-converter.job.v1",
        "job_id": "job-python-test",
        "correlation_id": "record-python-test",
        "source_system": "test",
        "attachments": [],
    }


class TestesAdaptadorPython(unittest.TestCase):
    """Valida publicação atômica e retenção de payload em falha."""

    def test_publica_ready_somente_apos_payload_e_manifesto(self) -> None:
        """Confirma que um job íntegro termina com manifesto, payload e READY."""
        adaptador = carregar_adaptador()
        with tempfile.TemporaryDirectory() as diretorio:
            raiz_spool = Path(diretorio)
            caminho_job = adaptador.publicar_job(
                raiz_spool,
                criar_manifesto(),
                [("payloads/attachment-0001.b64", "SGVsbG8=")],
            )

            self.assertTrue((caminho_job / "job.json").is_file())
            self.assertEqual(
                (caminho_job / "payloads" / "attachment-0001.b64").read_text(
                    encoding="utf-8"
                ),
                "SGVsbG8=",
            )
            self.assertTrue((caminho_job / "READY").is_file())

    def test_preserva_payload_quando_publicacao_falha(self) -> None:
        """Simula falha no manifesto e comprova retenção integral do Base64."""
        adaptador = carregar_adaptador()
        gravacao_original = adaptador.gravar_texto_duravel

        def falhar_no_manifesto(caminho: Path, conteudo: str) -> None:
            """Interrompe somente a escrita do manifesto após gravar o payload."""
            if caminho.name == "job.json":
                raise OSError("falha simulada de manifesto")
            gravacao_original(caminho, conteudo)

        adaptador.gravar_texto_duravel = falhar_no_manifesto
        with tempfile.TemporaryDirectory() as diretorio:
            raiz_spool = Path(diretorio)
            with self.assertRaisesRegex(RuntimeError, "staging preservado"):
                adaptador.publicar_job(
                    raiz_spool,
                    criar_manifesto(),
                    [("payloads/attachment-0001.b64", "SGVsbG8=")],
                )

            preservados = list((raiz_spool / "producer-failed").iterdir())
            self.assertEqual(len(preservados), 1)
            self.assertEqual(
                (
                    preservados[0] / "payloads" / "attachment-0001.b64"
                ).read_text(encoding="utf-8"),
                "SGVsbG8=",
            )
            self.assertFalse((preservados[0] / "READY").exists())


if __name__ == "__main__":
    unittest.main()
