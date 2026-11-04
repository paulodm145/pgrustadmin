# Instruções gerais para agentes

Leia este arquivo e os `AGENTS.md` locais antes de alterar código. Inspecione o repositório, especificações, ADRs e estado do Git.

## Princípios
- Rust stable, idiomático, simples, seguro e explícito.
- Não invente requisitos nem antecipe funcionalidades futuras.
- Não marque tarefas como concluídas sem implementação e validação.
- Não afirme que executou comandos que não executou.

## Arquitetura
- Axum concentra autenticação, autorização, conexões, metadados, execução SQL e arquivos.
- TUI/Web/Desktop são clientes da API no desenho atual.
- PostgreSQL é o único banco administrado no MVP.
- Banco de metadados separado dos bancos administrados.
- Não introduza microserviços, CQRS, event bus ou plugin system.
- Não crie traits/camadas genéricas sem necessidade concreta.

## Rust
Consulte `docs/development/rust-patterns.md`. Evite `clone()` para contornar borrow checker, `unwrap()`/`expect()` em caminhos normais, `unsafe`, `Arc<Mutex<T>>`, `Box<dyn Trait>` e genéricos sem justificativa.

## Segurança
Autorização sempre no servidor. Hash Argon2id para senhas de login; AEAD para credenciais externas, com chave fora do banco. Não registre segredos. SQL arbitrário é deliberado: não tente filtrá-lo por regex. SQL interno deve parametrizar valores. Bind em `127.0.0.1` por padrão; acesso remoto exige TLS e configuração explícita.

## Git
Use `feature/`, `fix/`, `docs/`, `test/`; commits Conventional Commits. Não faça merge em master, force push, apague branches, reescreva histórico compartilhado ou publique releases/tags sem autorização explícita.

## Entrega
Revise diff/status, atualize testes/docs e informe arquivos, decisões, comandos executados e limitações.
