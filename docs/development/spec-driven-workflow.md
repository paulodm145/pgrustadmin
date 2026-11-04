# Fluxo de desenvolvimento guiado por especificações

As especificações em `specs/` orientam o comportamento esperado. O backlog organiza a sequência; ele não substitui uma especificação nem prova que algo já foi implementado.

## Ciclo de uma tarefa

1. Escolher uma tarefa pendente em `tasks/initial-backlog.md` e confirmar suas dependências.
2. Abrir apenas a especificação relevante e os ADRs necessários para aquela tarefa. Registrar o caminho da especificação na tarefa antes de implementar.
3. Se faltar uma decisão ou critério verificável, atualizar a especificação ou o ADR primeiro. Não preencher lacunas com requisitos presumidos.
4. Implementar o menor escopo que satisfaça os critérios da especificação, seguindo os `AGENTS.md` locais.
5. Executar as validações adequadas ao escopo e registrar os comandos e resultados reais.
6. Revisar diff e estado do Git; atualizar a tarefa e a documentação afetada.

Tarefas sem implementação (auditorias e decisões) podem ser concluídas com evidência registrada, sem exigir testes de código. Não marcar uma implementação como concluída apenas porque existe documentação ou scaffold.

## Estados

- **Pendente:** ainda não iniciada.
- **Em andamento:** especificação e escopo da tarefa estão identificados.
- **Bloqueada:** depende de decisão ou pré-requisito explícito; registrar qual.
- **Concluída:** critérios satisfeitos e validações aplicáveis registradas.

## Escopo desta preparação

Por enquanto, as especificações não foram abertas. A auditoria inicial registra apenas a estrutura do repositório e deixa o vínculo a cada especificação para o início da respectiva tarefa.
