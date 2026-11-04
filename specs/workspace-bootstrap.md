# Especificação: bootstrap do workspace Rust

## Objetivo

Preparar a estrutura Rust inicial do PGRustAdmin para que o desenvolvimento do MVP possa começar sobre um workspace compilável.

## Escopo

- Workspace Cargo na raiz.
- Binários iniciais para `apps/server` e `apps/tui`.
- Bibliotecas iniciais para `crates/api-client`, `crates/app-core` e `crates/postgres-driver`.
- Configuração comum de edição Rust e exclusão de artefatos locais do Git.

Este bootstrap não implementa comportamento do produto, dependências de runtime, persistência, API nem interface de terminal. Web e Desktop permanecem fora do workspace Rust inicial.

## Critérios de aceite

1. Cargo reconhece um workspace virtual na raiz com os cinco componentes Rust listados.
2. Cada componente tem um manifesto e o alvo Rust correspondente (`lib.rs` para crates e `main.rs` para aplicações).
3. `cargo check --workspace` termina com sucesso sem exigir serviços externos ou baixar dependências de terceiros.
4. Os diretórios de build ficam ignorados pelo Git.
5. A configuração permanece alinhada com Rust stable e não introduz camadas ou comportamento ainda não especificados.
