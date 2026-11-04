# Padrões Rust

Referência: https://rust-unofficial.github.io/patterns/patterns/index.html

## Idioms úteis
Newtype para IDs; RAII/guards para recursos; borrowed types quando ownership não é necessário; decomposição de structs; `Option::take`, `mem::take` e `mem::replace` quando simplificam ownership; Builder apenas para configuração realmente complexa; crates separadas por responsabilidade real.

## Condicionais
Strategy/Policy somente com implementações alternativas reais. Command para operações enfileiráveis/auditáveis. Visitor, Interpreter, dispatch dinâmico e genéricos avançados somente com necessidade demonstrada. Não criar interpretador SQL nem framework de drivers futuro.

## Anti-patterns
- Clone to satisfy borrow checker: não clonar só para silenciar erro; avaliar referências, escopo, ownership e decomposição. Clone intencional é válido.
- Deref polymorphism: não implementar `Deref` para simular herança/expor campos; prefira métodos explícitos.
- `#![deny(warnings)]` indiscriminado: política de warnings deve ser controlada; CI usa Clippy `-D warnings`.

## Regras do projeto
Não criar trait para cada service/repository; não usar `Arc<Mutex<T>>` ou `Box<dyn Trait>` por padrão; não criar camadas pass-through; não usar `unsafe` para contornar ownership; evitar `unwrap/expect` em caminhos normais; não manter estado global mutável; SQL interno parametrizado; não validar SQL arbitrário por regex.

Toda abstração não trivial deve responder: problema concreto, alternativa simples, custo de complexidade e testes.
