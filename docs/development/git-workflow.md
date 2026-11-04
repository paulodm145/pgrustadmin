# Git workflow

Branches: `master`, `feature/<nome>`, `fix/<nome>`, `docs/<nome>`, `test/<nome>`. Fluxo: atualizar master, criar branch, implementar escopo pequeno, executar validações, revisar diff/status, commits atômicos e PR com contexto/testes/riscos. Aguardar aprovação antes de integrar.

Conventional Commits: `feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `chore:`. Sem autorização explícita, agentes não fazem merge em master, force push, reescrita de histórico compartilhado, exclusão de branches, releases ou tags.
