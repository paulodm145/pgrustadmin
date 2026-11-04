# Editor SQL

Cada aba mantém ID, conteúdo, arquivo, dirty, conexão/database, execução, resultado, duração e erro.

Arquivos: criar/abrir/salvar/salvar como `.sql` UTF-8; isolamento por usuário; servidor controla paths e bloqueia traversal; alertar ao fechar aba dirty; conflito de gravação não pode sobrescrever silenciosamente.

Execução: assíncrona; mostrar conexão/database; limite de linhas/volume; timeout e concorrência; cancelamento; separar resultados antigos/atuais; erro seguro. `EXPLAIN ANALYZE` exige confirmação.

SQL do usuário é arbitrário por definição. Não filtrar por regex. SQL interno parametriza valores.

Aceite: testes de abas, dirty tracking, arquivos, autorização, execução, limite, timeout, cancelamento e erros.
