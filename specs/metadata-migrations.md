# Especificação: migrations iniciais de metadados

## Objetivo

Criar a primeira migração versionada para o banco PostgreSQL dedicado a metadados e uma operação explícita para aplicá-la.

## Escopo inicial

- Usar migrations SQL versionadas do SQLx.
- Criar somente a tabela `users`, conforme AUTH-001: UUID v4, email canônico único, hash de senha, estado ativo e instante de criação UTC.
- O comando de migração é explícito: a aplicação nunca executa migrations automaticamente. O operador as aplica com `pgrustadmin-server migrate`. Ao iniciar, o servidor compara as migrations embutidas com as aplicadas com sucesso em `_sqlx_migrations` e, se faltar alguma, não sobe: termina com erro listando versão e descrição das pendentes e pedindo `pgrustadmin-server migrate`.
- O comando usa `PGRUSTADMIN_METADATA_DATABASE_URL`; o valor não pode aparecer nos logs nem ser gravado no repositório.
- Sessões, conexões, arquivos e preferências entram em migrations posteriores, quando suas especificações estiverem prontas.

## Limites

O comando atua apenas no banco de metadados dedicado de ADR-0002. Não executar migrations nos bancos administrados pelo usuário. Erros de conexão e migração devem indicar a operação que falhou sem revelar a URL.

## Critérios de aceite

1. A migration versionada cria `users` com UUID como chave primária, email não vazio e único, hash não vazio, estado e `TIMESTAMPTZ` de criação.
2. O comando `migrate` exige `PGRUSTADMIN_METADATA_DATABASE_URL` e aplica migrations explicitamente.
3. A inicialização normal pode conectar ao banco de metadados para atender requisições, mas não executa nem modifica migrations.
4. `cargo fmt --all -- --check`, `cargo check --workspace` e Clippy sem warnings passam.
