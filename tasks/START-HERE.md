# Comece por aqui

Consulte `docs/development/spec-driven-workflow.md` para o ciclo de cada tarefa.

## Próxima sequência

1. PREP-001 — auditoria inicial concluída. Naquele momento, o repositório continha planejamento e configuração inicial de infraestrutura, sem workspace Rust.
2. PREP-006 — workspace Rust mínimo concluído. Especificação e validações registradas no backlog.
3. PREP-002 — revisão estática da configuração PostgreSQL local concluída; o serviço foi iniciado depois para validar PREP-004.
4. PREP-003 — configuração, logs e falhas de inicialização concluídos; detalhes no backlog.
5. AUTH-001 — modelo de usuário definido; decisões registradas em ADR-0002 e `specs/metadata-schema.md`.
6. PREP-004 — migrations iniciais aplicadas ao PostgreSQL local; detalhes em `specs/metadata-migrations.md`.
7. PREP-005 — workflow CI configurado em `.github/workflows/ci.yml` e especificado em `specs/ci.md`.
8. AUTH-002 — Argon2id concluído conforme `specs/authentication.md`.
9. AUTH-004 — armazenamento de sessões expiráveis e revogáveis concluído.
10. AUTH-003 — login/logout e validação de sessão Bearer concluídos; próxima tarefa: AUTH-005, autorização por recurso.

Ao iniciar cada tarefa, identificar sua especificação, dependências e critérios verificáveis. Registrar bloqueios e validações reais no backlog. Não implementar enquanto houver decisão necessária sem resposta.
