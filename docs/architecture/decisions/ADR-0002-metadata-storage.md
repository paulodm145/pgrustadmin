# ADR-0002 — Banco de metadados
- Status: Aceito
- Data: 2026-11-02

## Contexto
Usuários, sessões, conexões, metadados de arquivos e preferências não devem se misturar aos bancos administrados.

## Decisão
Instância/base PostgreSQL dedicada para metadados no MVP.

## Alternativas
SQLite simplifica instalação local, mas complica servidor multiusuário; compartilhar banco administrado cria acoplamento; PostgreSQL dedicado separa responsabilidades e reutiliza tecnologia central.

## Consequências
Instalação exige PostgreSQL de metadados e rotina de backup. A escolha foi confirmada em 2026-11-04. PREP-004 criou e aplicou migrations SQLx iniciais; operações explícitas usam `PGRUSTADMIN_METADATA_DATABASE_URL`.

## Decisões de modelo inicial
- IDs de usuários: UUID versão 4.
- Email: remover espaços nas extremidades e converter para minúsculas antes de armazenar e comparar; normalização não substitui validação de formato na API.
