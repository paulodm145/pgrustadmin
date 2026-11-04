# Conexões PostgreSQL

CRUD autorizado por proprietário. Campos mínimos: nome, host, porta, database padrão, usuário, TLS e credencial. Senha nunca é retornada após gravação; credencial cifrada em repouso conforme ADR-0003. Teste de conexão com timeout e erro sanitizado. Não registrar connection string com senha.

Aceite: CRUD isolado por usuário; teste distingue sucesso/falha sem vazar segredo; timeout aplicado; integração testada com PostgreSQL real.

Em aberto: compartilhamento, opções TLS e rotação.
