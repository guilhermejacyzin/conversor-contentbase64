"""Testes do gate que impede dados privados no repositorio publico."""

from __future__ import annotations

import importlib.util
import base64
import sys
import unittest
from pathlib import Path


def carregar_modulo_auditoria():
    """Carrega o script como modulo sem exigir um pacote Python em scripts."""
    caminho = Path(__file__).resolve().parents[1] / "scripts" / "audit_public_repo.py"
    especificacao = importlib.util.spec_from_file_location("auditoria_publica", caminho)
    if especificacao is None or especificacao.loader is None:
        raise RuntimeError("nao foi possivel carregar o modulo de auditoria")
    modulo = importlib.util.module_from_spec(especificacao)
    sys.modules[especificacao.name] = modulo
    especificacao.loader.exec_module(modulo)
    return modulo


class AuditoriaPublicaTests(unittest.TestCase):
    """Valida deteccao textual, binaria e de cargas Base64 ocultas."""

    @classmethod
    def setUpClass(cls) -> None:
        """Prepara um unico conjunto de padroes para todos os cenarios."""
        cls.modulo = carregar_modulo_auditoria()
        cls.padroes = cls.modulo.criar_padroes_proibidos()

    def test_detecta_termo_reservado_com_variacao_de_caixa(self) -> None:
        """Confirma que um nome reservado nao passa por mudanca de caixa."""
        termo = "private-sample-marker"
        padroes = self.modulo.criar_padroes_proibidos([termo])
        ocorrencias = self.modulo.auditar_texto(
            termo.upper(),
            "teste",
            padroes,
        )
        self.assertTrue(
            any(item.regra.startswith("external-reserved-term-") for item in ocorrencias)
        )

    def test_detecta_binario_versionado(self) -> None:
        """Confirma que um artefato binario exige revisao explicita antes do commit."""
        ocorrencias = self.modulo.auditar_bytes(
            b"\x00\xff\x01",
            "fixture.bin",
            ".bin",
            self.padroes,
        )
        self.assertTrue(
            any(item.regra == "binary-artifact-not-allowed" for item in ocorrencias)
        )

    def test_detecta_termo_reservado_dentro_de_base64(self) -> None:
        """Confirma que a codificacao Base64 nao consegue ocultar um nome reservado."""
        termo = "private-sample-marker"
        carga = base64.b64encode(termo.encode("ascii"))
        padroes = self.modulo.criar_padroes_proibidos([termo])
        ocorrencias = self.modulo.auditar_base64(
            carga,
            "fixture.b64",
            padroes,
        )
        self.assertTrue(
            any(item.regra.startswith("external-reserved-term-") for item in ocorrencias)
        )

    def test_detecta_termo_reservado_em_lista_numerica(self) -> None:
        """Confirma que listas de codigos ASCII nao ocultam termos externos."""
        termo = "private-sample-marker"
        codigos = ", ".join(str(valor) for valor in termo.encode("ascii"))
        textos = self.modulo.extrair_textos_de_listas_numericas(f"[{codigos}]")
        padroes = self.modulo.criar_padroes_proibidos([termo])
        ocorrencias = self.modulo.auditar_texto(
            "\n".join(textos),
            "fixture.py",
            padroes,
        )
        self.assertTrue(
            any(item.regra.startswith("external-reserved-term-") for item in ocorrencias)
        )

    def test_aceita_fixture_textual_generica(self) -> None:
        """Confirma que um exemplo sintetico e textual continua permitido."""
        ocorrencias = self.modulo.auditar_base64(
            b"SGVsbG8sIEJhc2U2NCEK",
            "fixture.b64",
            self.padroes,
        )
        self.assertEqual([], ocorrencias)


if __name__ == "__main__":
    unittest.main()
