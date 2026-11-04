# Visão do produto

PGRustAdmin é um gerenciador PostgreSQL para desenvolvedores e administradores que precisam navegar estruturas e executar SQL, inclusive em ambientes Linux/SSH sem desktop.

## Proposta
Uma API/servidor Axum concentra acesso a dados e segurança. A primeira interface é TUI Ratatui. Web React/TypeScript e Desktop Tauri 2 são futuras.

## MVP
1. Autenticação, sessões e autorização.
2. CRUD e teste de conexões PostgreSQL.
3. Catálogo: databases, schemas, tabelas/views, colunas e índices.
4. Editor SQL com múltiplas abas.
5. Abrir/criar/salvar/salvar como `.sql` UTF-8.
6. Execução com limites, timeout, resultados e cancelamento.
7. TUI acessível por teclado.

## Fora do MVP
MySQL/SQLite, colaboração, plugins, alta disponibilidade, Web/Desktop completos e administração avançada não priorizada.

## Princípios
Segurança por padrão, feedback claro, SQL explícito, operação simples e interfaces consistentes.
