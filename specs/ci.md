# Especificação: verificações contínuas

## Objetivo

Executar as verificações Rust do repositório em cada push e pull request.

## Critérios de aceite

1. O workflow roda em push e pull request usando Rust stable.
2. Ele executa, nesta ordem, `cargo fmt --all -- --check`, `cargo test --workspace` e `cargo clippy --workspace --all-targets -- -D warnings`.
3. Qualquer comando com falha reprova o job.
4. O workflow não requer serviços, credenciais ou segredos.
