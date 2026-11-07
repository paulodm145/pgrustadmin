# Interface de terminal

A TUI usa a API HTTP como fonte de autenticação e dados. Login é a primeira tela; senha não aparece em texto, nem em mensagens de erro. Login e validação de sessão ocorrem fora do loop de eventos, que continua processando desenho e saída.

TUI-002 envia email/senha ao endpoint login e consulta a sessão autenticada antes de mostrar o estado conectado. Credenciais inválidas e falhas de transporte apresentam mensagens curtas sem corpo HTTP, stack trace ou segredo. A senha temporária é zerada ao sair de escopo. O usuário precisa ter conta ativa provisionada no banco de metadados; cadastro de usuários não faz parte deste item.

O formulário navega entre email e senha por Tab/Shift+Tab e setas, aceita Enter para avançar/enviar, mascara a senha e permite sair por Esc ou Ctrl+C. Enquanto a API responde, a tela continua redesenhando e aceita saída.

TUI-003 lista até 100 conexões por página, permite navegar páginas e criar, editar, testar e excluir a conexão selecionada. Criação/edição usa os campos do contrato de conexões; a senha permanece mascarada e vazia na edição preserva a credencial existente. Exclusão exige confirmação. Chamadas à API rodam fora do loop, com mensagens de carregamento/erro/sucesso sem segredos; a lista é recarregada após mutações.

TUI-004 aplica os temas escuro (padrão) e claro de `docs/design-system/tokens.md`, com cores semânticas validadas por contraste. `PGRUSTADMIN_TUI_THEME=dark|light` (sem diferenciar maiúsculas) define o tema inicial; valor desconhecido usa o escuro e mostra um aviso. `F2` alterna o tema em qualquer tela durante a sessão, sem persistir e sem digitar texto. A capacidade de cor é detectada sem presumir true color: `NO_COLOR` não vazio desativa cores (foco e seleção por negrito/reverso); `COLORTERM=truecolor|24bit` usa RGB exato; caso contrário, cada cor vira a mais próxima da paleta de 256 cores. Estado nunca é comunicado só por cor: mensagens trazem texto (`Failed`, `Cancelled`, `*` de dirty). O rodapé tem três linhas para que a borda não esconda atalhos e confirmações.
