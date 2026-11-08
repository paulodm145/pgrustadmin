# ADR-0005 — Edição de dados pela grade
- Status: Aceito
- Data: 2026-11-08

## Contexto
A visão do produto prioriza SQL explícito. Usuários pediram para alterar valores direto na grade da prévia de tabelas, com editores por tipo (boolean, enum, data/hora, chave estrangeira).

## Decisão
- A grade da prévia de uma tabela pode editar dados; o resultado do editor SQL continua só leitura, porque o SQL do usuário não é analisado e suas linhas não têm origem confiável.
- Só tabelas comuns com chave primária são editáveis. A chave identifica o registro; views e tabelas sem chave ficam só leitura.
- Alterações se acumulam na TUI como pendentes e são gravadas juntas, numa única transação, por um comando explícito; outro comando descarta. Se qualquer linha falhar, nada é gravado.
- O servidor gera SQL interno parametrizado, com identificadores vindos do catálogo e entre aspas. Cada `UPDATE` confere também os valores originais das colunas alteradas; registro alterado por outra sessão faz a gravação falhar sem sobrescrever.
- Cada conexão tem a opção "somente leitura" (padrão: desligada). Com ela, o servidor recusa gravações pela grade e a TUI não entra em edição. A opção não afeta o editor SQL, que executa o SQL explícito do usuário.
- A permissão do papel no PostgreSQL continua sendo a autoridade final.

## Consequências
Novas rotas de gravação e uma migration em `connections`. Editores por tipo, inserção e remoção de linhas vêm em tarefas seguintes (EDIT-002 a EDIT-004).
