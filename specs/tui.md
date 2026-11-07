# Interface de terminal

A TUI usa a API HTTP como fonte de autenticação e dados. Login é a primeira tela; senha não aparece em texto, nem em mensagens de erro. Login e validação de sessão ocorrem fora do loop de eventos, que continua processando desenho e saída.

TUI-002 envia email/senha ao endpoint login e consulta a sessão autenticada antes de mostrar o estado conectado. Credenciais inválidas e falhas de transporte apresentam mensagens curtas sem corpo HTTP, stack trace ou segredo. A senha temporária é zerada ao sair de escopo. O usuário precisa ter conta ativa provisionada no banco de metadados; cadastro de usuários não faz parte deste item.

O formulário navega entre email e senha por Tab/Shift+Tab e setas, aceita Enter para avançar/enviar, mascara a senha e permite sair por Esc ou Ctrl+C. Enquanto a API responde, a tela continua redesenhando e aceita saída.
