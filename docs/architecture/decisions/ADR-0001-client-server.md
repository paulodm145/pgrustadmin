# ADR-0001 — Arquitetura cliente-servidor
- Status: Aceito como direção do MVP
- Data: 2026-11-02

## Contexto
TUI inicial e interfaces futuras precisam compartilhar regras e acesso a dados sem duplicar segurança.

## Decisão
Axum concentra autenticação, autorização, conexões, metadados, execução SQL e arquivos. TUI, Web e Desktop são clientes da API.

## Consequências
Segurança centralizada e clientes desacoplados; exige operar um servidor. TUI poderá conectar a servidor local ou remoto. Ciclo de vida/inicialização local ainda precisa de especificação. Desktop standalone fica fora desta decisão.
