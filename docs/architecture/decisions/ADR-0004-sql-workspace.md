# ADR-0004 — Workspace SQL
- Status: Aceito como requisito funcional
- Data: 2026-11-02

Cada aba mantém ID, SQL, arquivo, dirty state, conexão/database, estado da execução, resultado, duração e erro. Criar/abrir/salvar/salvar como `.sql` UTF-8. Arquivos isolados por usuário; servidor controla paths e bloqueia traversal. Definir controle de conflito para evitar sobrescrita silenciosa. TUI alerta sobre alterações não salvas.
