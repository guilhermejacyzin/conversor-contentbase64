//! Ajustes mínimos de link necessários por dependências nativas opcionais.

/// Declara bibliotecas do sistema exigidas pelo extrator RAR no Windows.
fn main() {
    #[cfg(target_os = "windows")]
    println!("cargo:rustc-link-lib=advapi32");
}
