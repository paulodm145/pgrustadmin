# Design tokens

Identidade técnica e discreta; brand teal `#005F6A`.

## Escuro (padrão)
| Token | Valor |
|---|---|
| background | `#17191F` |
| surface | `#222631` |
| surface-elevated | `#292F3B` |
| text-primary | `#F3F5F7` |
| text-secondary | `#AAB2BF` |
| brand | `#005F6A` |
| brand-hover | `#087985` |
| border | `#343B49` |

## Claro
| Token | Valor |
|---|---|
| background | `#F5F7FA` |
| surface | `#FFFFFF` |
| surface-secondary | `#EDF1F5` |
| text-primary | `#202631` |
| text-secondary | `#596575` |
| brand | `#005F6A` |
| border | `#D9E0E8` |

## Semânticos e foco (TUI-004)

Contraste WCAG validado por teste em `apps/tui/src/theme.rs`: texto e cores semânticas ≥ 4,5:1 sobre background e surface; texto da seleção ≥ 4,5:1 sobre a seleção; borda de foco ≥ 3:1 sobre background.

| Token | Escuro | Claro |
|---|---|---|
| success | `#5CC98F` | `#17703F` |
| warning | `#E8B85A` | `#8A5500` |
| error | `#F2787A` | `#B42318` |
| info / accent | `#6CB6E0` | `#0B5F96` |
| selection (fundo) | `#005F6A` | `#005F6A` |
| selection-text | `#F3F5F7` | `#FFFFFF` |
| focus-border | `#087985` | `#005F6A` |

No tema escuro, `brand` (`#005F6A`) tem 2,38:1 sobre o background e não é usado como cor de texto; aparece como fundo da seleção.

Não comunicar estado somente por cor. Reservar brand para identidade/ações e manter estados disabled legíveis.
