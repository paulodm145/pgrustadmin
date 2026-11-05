# Esquema de metadados

Entidades previstas: `users` (UUID v4, email normalizado, hash Argon2id, estado); `sessions` (UUID, hash SHA-256 de token aleatório único, usuário, criação, expiração fixa em 12 horas, revogação); `connections` (proprietário, parâmetros, credencial cifrada); `sql_files` (proprietário, nome, storage key, tamanho, hash); `user_preferences` se necessárias.

Email é normalizado removendo espaços externos e convertendo para minúsculas. A normalização não valida o formato; essa validação pertence à API.

IDs tipados no domínio; FKs/índices por proprietário; email normalizado único; nunca senha em texto puro; credenciais externas cifradas; timestamps UTC; migrations versionadas. DDL final depende do ADR-0002, sessão e envelope criptográfico.
