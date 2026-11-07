# Editor SQL

Cada aba mantém ID, conteúdo, arquivo, dirty, conexão/database, execução, resultado, duração e erro.

Arquivos: criar/abrir/salvar/salvar como `.sql` UTF-8; isolamento por usuário; servidor controla paths e bloqueia traversal; alertar ao fechar aba dirty; conflito de gravação não pode sobrescrever silenciosamente.

Execução: assíncrona; mostrar conexão/database; limite de linhas/volume; timeout e concorrência; cancelamento; separar resultados antigos/atuais; erro seguro. `EXPLAIN ANALYZE` exige confirmação.

SQL do usuário é arbitrário por definição. Não filtrar por regex. SQL interno parametriza valores.

Aceite: testes de abas, dirty tracking, arquivos, autorização, execução, limite, timeout, cancelamento e erros.

## SQL-001 — Estado das abas

As abas vivem apenas na memória da TUI e somem ao sair; persistência de conteúdo vem com arquivos `.sql` (SQL-003). Cada aba tem ID numérico único na sessão (nunca reutilizado), SQL (vazio ao abrir), arquivo associado (nenhum ao abrir), estado dirty (falso ao abrir), conexão (ID e nome) e database. O vínculo com conexão/database é fixo na aba. Execução, resultado, duração e erro entram no estado da aba com SQL-004/SQL-005, quando passam a ter comportamento; SQL-001 não os modela.

No máximo 20 abas ficam abertas ao mesmo tempo; abrir a 21ª é recusado com mensagem, sem alterar as abas existentes. Abrir uma aba a torna ativa. Fechar a aba ativa ativa a aba seguinte (ou a anterior, se era a última); fechar a última aba volta à lista de conexões. Alternar entre abas é circular.

Na TUI, `s` na lista de databases abre nova aba ligada à conexão e ao database selecionados e mostra a seção SQL. Nas telas Overview e Connections, `s` mostra a seção SQL quando houver abas abertas. Na seção SQL, a barra de abas mostra cada aba com número, database, conexão e marcador de dirty, e destaca a ativa; `[`/`]` alternam a aba ativa, `x` fecha a aba ativa e Tab/Esc voltam à lista de conexões. Edição do SQL, dirty tracking e alerta ao fechar aba dirty pertencem a SQL-002.
