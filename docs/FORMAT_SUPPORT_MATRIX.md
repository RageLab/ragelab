# Matriz técnica de formatos — GTA V Legacy / FiveM

**Data da auditoria:** 2026-10-31
**Alvo ativo:** GTA V Legacy / Gen8 PC e fluxos de resource FiveM.
**Autoridade:** crates Rust especializadas + `ragelab-engine`.
**Esta matriz descreve o código existente; itens planejados não são apresentados como suporte implementado.**

Referências de implementação: [ARCHITECTURE.md](ARCHITECTURE.md), [CLI_CONTRACT.md](CLI_CONTRACT.md), [CAPABILITIES.md](CAPABILITIES.md), [EXPORT.md](EXPORT.md), [GLB_IMPORT.md](GLB_IMPORT.md), [CLI_CORE_STUDIO_ARCHITECTURE.md](CLI_CORE_STUDIO_ARCHITECTURE.md).

## 1. Semântica dos estados

Os estados abaixo descrevem a evidência observada no repositório, não uma promessa genérica de compatibilidade com qualquer variante de GTA/FiveM.

| Estado | Significado |
|---|---|
| **Implementado — escopo delimitado** | Parser/operation e comandos ou APIs estão presentes, com regras e limites identificados. Não significa suporte a todas as variantes do formato. |
| **Condicional por asset** | A operação existe, mas só é permitida quando `capabilities` e o writer específico comprovam elegibilidade para aquele arquivo, índice, binding e parâmetros. |
| **Read-only** | Há parser e inspeção/validação específica, mas não há writer público para editar ou criar esse formato. |
| **Interno** | A crate tem API útil, porém o uso atual é parte de outro fluxo (container, subresource, serializer interno, template). Não há operação pública independente equivalente. |
| **Container-only** | Só a camada RSC7/RPF/etc. foi reconhecida. Isso não comprova parsing semântico do formato interno. |
| **Não implementado** | Não há parser/writer ou contrato público para a operação. |

**Regra:** reconhecimento da extensão, parsing do container e parsing semântico do formato são capacidades distintas. Nunca exibir container-only como format-validated.

## 2. Matriz resumida por formato

| Formato | Parser / versão observada | Inspect | Validate | Extract / preview | Edit / create / convert | Situação real |
|---|---|---|---|---|---|---|
| **GLB 2.0** | `ragelab-gltf`: container, scenes/nodes, meshes/primitives, accessors, attributes, materials, textures/images | `glb inspect`, `glb geometry`, profile init/validate | Profile/source/reference/feature gates integrados ao export; ainda não há `glb validate` geral prévio | Geometria estática e PNGs embutidos/data URI decodificados para export | Profile authoring; export estático para YDR v165; 5 signatures ShaderFX exatas opcionais | Implementado — input pipeline delimitado |
| **YDR** | `ragelab-ydr`: Drawable/Model/Geometry, vertex declarations, shaders, bindings, embedded YTD; RSC7 Legacy PC | `inspect`, `ydr.info`, material authoring report, preview | `validate` genérico abre RSC7 + YDR parser; semantic reopen nos writers que o executam | Preview renderer-neutral de geometria; textura embutida lida via TextureDictionary | Writer de criação para YDR estático; `ydr.translate`, `ydr.rebind-texture`, `ydr.rebind-shader` condicionais | Implementado — escopo delimitado/condicional |
| **YDD** | `ragelab-ydd`: dictionary Legacy v165 e Drawables lidos por `ragelab-ydr` | `inspect`, `ydd.info`, bindings por drawable | Container + dictionary parser; sessions validam drawable selecionado | Preview exige `drawableIndex` explícito | `ydd.translate`, `ydd.rebind-texture`, `ydd.rebind-shader` condicionais por drawable | Implementado — escopo delimitado/condicional |
| **YTD** | `ragelab-ytd`: TextureDictionary Legacy v13/RSC7, textura e mip payloads | `inspect`, `ytd.info`, relatório de authoring por textura | Container + YTD parser; writes reabrem e verificam semântica | Export DDS; export PNG top mip; decode RGBA de mips | Substituição/repack DDS/PNG/RGBA e rebuild compact; apenas 2D e formatos/contratos indicados abaixo | Implementado — escopo delimitado/condicional |
| **YBN** | `ragelab-ybn`: collision bounds, geometry, child transforms, material/polygon records; versão Legacy PC suportada pela crate | `inspect`, `ybn.info`, collision preview | Container + collision parser | Preview renderer-neutral de colisão | `ybn.edit-polygon`: patch layout-preserving de radius existente e material slot do mesmo child; sem topologia | Implementado — edição muito restrita |
| **YMAP** | `ragelab-ymap` + `ragelab-meta`: META/RSC7 placements, entities, transforms e referências | `inspect`, `ymap.info`, spatial, workspace scene/deps/preflight | Container + YMAP/META parser; writer faz semantic reopen | Não extrai geometria por si; workspace scene resolve referências e preview/render usa outros assets | `ymap.set-transform`, `set-properties`, `delete`, `duplicate`, `create-from-template` via operation document; Studio possui `YmapAuthoringSession`/undo/redo/save-as; workspace export | Implementado — authoring de placement/workspace, não de modelos |
| **YTYP** | `ragelab-ytyp` + `ragelab-meta`: archetypes, MLO, rooms/portals/entity sets e dependencies | `inspect`, `ytyp.info`, spatial/MLO audit via engine | Container + YTYP/META parser | Resolução de dependências via workspace, não export de payload de forma independente | Nenhum writer/authoring público dedicado observado | Read-only |
| **YMF** | `ragelab-ymf` sobre PSO/PSCH | `inspect`, `ymf.info` | Container/PSO/YMF parser | Usado como metadata/dependency manifest em workspace export | `to_bytes` existe e é usado para gerar manifest no export; não há fluxo público autônomo de authoring | Read-only como comando standalone; writer interno |
| **YFT** | `ragelab-yft`: Legacy v162, lê FragType e apenas o `main fragDrawable` através do parser YDR | `inspect` com escopo `legacyV162MainDrawable`, `yft.info` | `validate` integra `YftDocument::from_bytes`; verifica FragType v162 e o main drawable quando presente | Não há preview completo de fragment/physics; somente o main drawable é inspecionado; physics children, damaged variants e articulation permanecem fora do contrato | Read-only; serve de origem somente leitura para clone de ShaderFX; sem writer | Read-only / parser estreito |
| **RPF7** | `ragelab-rpf` em cima de `rage-rpf`: archive/directory entries, nested archives, keys e provenance | `rpf.info`, `rpf.list`, `rpf.keys`, GTA archive order/index | Não há comando standalone `rpf validate`; open/list/read verifica estrutura necessária à operação, não todos os payloads internos | `rpf.extract`, lookup/read nested entries | Sem writer/repack público | Read-only |
| **RSC7** | `ragelab-resource`: header, page flags, system/graphics segments, pointer reads/writes e bytes rebuild | Probe de container e metadados usados por inspect | Container/decompress/page/range checks | Não é um formato de asset sem a estrutura tipada interna | `Rsc7Resource::from_segments` / `to_bytes` são infraestrutura de writer das crates; não significa conversor arbitrário RSC7 | Interno / container layer |
| **META** | `ragelab-meta`: MetaDocument, blocks, structures, pointers, arrays e typed fields | Exposto indiretamente por YMAP/YTYP e comandos de informação | Validado em conjunto com o formato dono | Não é recurso autônomo de asset | `rewrite_blocks` usado por writers específicos; sem editor META genérico público | Interno / subresource |
| **PSO / PSCH** | `ragelab-pso`: parse e construção dos blocos/estruturas PSO/PSCH | Exposto através de YMF | Parser ao abrir YMF | N/A | Builder usado para serializar YMF; sem comandos PSO/PSCH genéricos | Interno / subresource |
| **RBF** | `ragelab-rbf`: parser de RBF structure/value | Sem comando público específico de inspeção RBF no dispatch atual | Não há contrato autónomo de validação de asset RBF | N/A | Parser only; sem writer público observado | Interno/read-only |
| **PNG** | Decodificação PNG RGBA para import/repack de textura; PNG é payload, não resource GTA | Metadata de imagem via GLB/YTD reports | MIME/pixels/dimensões/ranges verificados no consumidor | Export de top mip YTD; imagens GLB embutidas decodificadas | PNG para YTD RGBA8/BC1/BC3 com mip chain, sujeito ao target; GLB export inclui PNG embutido | Payload suportado — escopo delimitado |
| **DDS clássico** | Decoder/parser do payload DDS na crate YTD | Reporte indireto em capabilities YTD | Header, dimensões, format, mip count e payload length verificados no replace/repack | Export de texture payload YTD para DDS | Replace layout-preserving ou relocated, desde que o formato Legacy do target seja mantido | Payload suportado — condicional |
| **OBJ / FBX / glTF JSON** | Nenhuma crate/parser de importação correspondente identificada nesta auditoria | Não implementado | Não implementado | Não implementado | Não implementado | Não suportado atualmente |

## 3. Limites relevantes por formato

### GLB → YDR

A rota existente é um import/export, não um conversor PBR universal.

- Entrada atual: GLB 2.0, cena estática, um High LOD, imagens PNG embutidas em BIN bufferView ou data URI PNG.
- Geometria: POSITION, NORMAL, TANGENT, COLOR_0, TEXCOORD_0, TEXCOORD_1 e TEXCOORD_2; conversão de coordenadas, transforms hierárquicos, seleção de subárvores por `glb tree`/`--node` e modos triangles/strip/fan são tratados pelo pipeline GLB/Core.
- Imagens: JPEG, MIME diferentes, external images e external buffers não são aceitos por esta rota.
- Não implementado: skinning, morph targets, animação, múltiplos LODs e UV sets acima de 2.
- ShaderFX: cinco assinaturas Legacy exatas: `normal.sps`, `spec.sps`, `normal_spec_tnt.sps`, `emissivenight.sps`, `emissive.sps`. Templates YFT/YDR de entrada são read-only. Material PBR não representável é rejeitado.
- O writer faz semantic reopen. Exports sem `--shader-template` são estruturais e não seguros para renderizar no CodeWalker Model Viewer; use um template Legacy verificado. A aceitação runtime permanece um gate separado.
- Evidência externa observada: CodeWalker Core parse dos outputs individuais (28/28); um YDR corrigido representativo foi confirmado pelo usuário na GUI do CodeWalker. Cinco fixtures ShaderFX foram aceitas no FiveM com confirmação do usuário. Os 28 outputs individuais novos ainda não foram abertos um a um na GUI. GTA V Legacy standalone continua WAIVED/NOT RUN.

O use case byte-first GLB→YDR reside em `ragelab-engine`; `glb tree` e `glb export --node` são adapters CLI sobre esse Core. Studio chama os mesmos serviços Engine através do Tauri; não há orquestração de formato paralela na CLI.

### YDR / YDD

- YDR writer de criação atual destina-se a geometria estática Legacy v165, índices/vertex ranges delimitados e TextureDictionary embutido.
- `YdrEditSession::rigid_translation_capability` só habilita tradução quando o layout audit está íntegro, há exatamente um LOD ativo e um model, há provenance de geometry e vertex storage é disjunto.
- Edição de posições individuais de vértices permanece indisponível porque normal recomputation e invariantes de culling ainda não estão implementados.
- Rebind de texture/shader é indexado por binding/geometry e depende das referências existentes e checks de compatibilidade; não é substituição arbitrária de shaders.
- YDD é um dictionary: operações aplicam-se a um drawable selecionado, preservando identidade/entries do dictionary. Não há criação arbitrária de entradas/drawables novos.

### YTD

- Export DDS é disponível apenas para textura 2D em formato compatível.
- PNG export decodifica top mip, não o arquivo original com toda a mip chain.
- Replace DDS layout-preserving exige mesma dimensão, formato, mip count e encoded byte length.
- Relocated DDS pode alterar dimensões/mips, mas mantém o formato Legacy do target.
- PNG/RGBA repack só aceita targets 2D RGBA8, BC1 ou BC3; gera mip chain completa determinística e semantic-reopen.
- Compact rebuild reconstrói layout RSC7 Legacy v13 a partir de fontes conhecidas; não é "repair" genérico de qualquer YTD corrompido.

### YBN

- Edição não cria/remove polígonos, children, vértices ou índices e não muda topologia.
- Repack só altera campos existentes de radius suportado e/ou polygon-material byte existente, com o material limitado ao child correto.
- Box não expõe radius. Polygon kind precisa corresponder ao `expected_kind`; tipos não reconhecidos são rejeitados.
- Writer reabre o output e compara dados normalizados não-alvo.

### YMAP / YTYP / YMF

- YMAP authoring atual permite editar placements/metadata via comandos tipados; criação é baseada em template, não um authoring livre de archetypes/modelos.
- Workspace export resolve closure e faz gate de unresolved dependencies; gerar `fxmanifest.lua`, `_manifest.ymf` e metadata auxiliar não significa que a CLI edita genericamente todos os formatos incluídos.
- YTYP é necessário para archetypes/MLO e authoring contextual, mas continua read-only nesta superfície.
- YMF tem serializer, mas a geração autônoma de manifest não é uma operação de authoring publicada. Seu writer é hoje consumido pelo pipeline de export.

### YFT / RPF

- YFT parser é propositalmente estreito: Legacy v162 e `main fragDrawable`; physics children, damaged variants e articulation estão explicitamente fora do contrato atual.
- RPF reader oferece list/read e extração de conteúdo autorizado com keys; nenhuma operação altera ou repacota o archive original. RPF discovery/index/cache são serviços de engine, não writer de RPF.
- RPF open/read não implica que todos os recursos dentro do arquivo serão validados. Cada arquivo extraído precisa passar pelo parser correspondente.

## 4. Validação container-only e capabilities — correção implementada

O dispatch interno agora resolve extensões por um enum `AssetFormat` e reutiliza a validação semântica específica como gate das operações anunciadas. Os parsers de formato continuam sendo a autoridade de cada crate; o enum do engine não substitui seus contratos de versão/layout.

- Formatos com parser semântico integrado: YMAP, YTYP, YMF, YDR, YDD, YTD, YBN e YFT.
- `AssetValidationReport` preserva `valid` e `checks`, acrescentando os campos aditivos `containerValid` e `formatValidated`. `valid: true` só é emitido após o parser específico concluir com sucesso. Para RSC7 validado, `containerValid: true`; quando não há container RSC7, o campo é `null`.
- YCD, YMT, YLD e YNV permanecem sem parser semântico. Um RSC7 válido pode habilitar `inspect` em modo explicitamente container-only, com `reason` informativo; `validate` é `unavailable` em capabilities e o comando de validação retorna `unsupported` / exit code 3. Não há falso `valid: true`.
- YFT é validado e inspecionado somente pelo contrato existente de Legacy v162: FragType e `mainDrawable` opcional, interpretado pelo parser YDR com layout YFT. Physics children, damaged variants e articulation continuam fora do contrato; não há writer anunciado.
- YMAP agora anuncia `plan`, `apply` e `ymap.set-transform`, `ymap.set-property`, `ymap.delete`, `ymap.duplicate`, `ymap.create`. Cada operação individual reporta seus parâmetros obrigatórios e estado `parameterized`; `plan` é read-only, `apply` é write. As operações workspace continuam explicitamente `contextRequired`.

As regressões do engine constroem um RSC7 sintético válido sob extensões sem parser e verificam o estado container-only, exercitam YFT sem mainDrawable e comparam os IDs/parâmetros YMAP publicados com o dispatcher de authoring. A suíte de operações já cobre plan/apply YMAP não destrutivo com semantic reopen. A CLI mantém exit code 3 para ausência de parser e 4 para falha de validação de um parser conhecido.

O enum é a registry tipada de identidade/suporte semântico usada por inspect, validate e capabilities; a elegibilidade detalhada dos writers continua condicionada aos dados do asset e aos parâmetros/contexto de cada operação.

## 5. Dependências entre formatos e workflows

| Workflow | Cadeia de formatos / crates | Regra de dependência |
|---|---|---|
| Import de modelo | GLB → `ragelab-gltf` → `ragelab-ydr` → YDR v165 + embedded TextureDictionary | Shader template pode vir de YFT/YDR read-only; não escolher shader heurísticamente |
| YDR com texturas | YDR → RSC7 + Drawable/ShaderGroup → YTD TextureDictionary | Parser YDR expõe dicionário embutido; YTD mantém regras de formato próprias |
| YDD | YDD → RSC7 + entries → Drawables YDR compartilhados | Qualquer operação requer drawable index e preserva dictionary metadata |
| YTD authoring | YTD → RSC7 → texture descriptors/payload + DDS/PNG/RGBA | Replace precisa respeitar formato, depth, mips, ranges e paginação |
| YMAP world | YMAP → RSC7/META + YTYP dependencies + referenced assets | Spatial/workspace resolution exige explicit workspace/RPF mounts/index; isolado não inventa world position |
| FiveM resource | Workspace YMAP roots → dependency closure de YMAP/YTYP/assets → stream + YMF/GTXD + fxmanifest | Unresolved external dependencies bloqueiam export por default |
| RPF browsing | RPF7 → directory entries/nested RPF → raw bytes → parser tipado de asset | RPF index/read é uma etapa; não substitui validação semântica do asset interno |
| YFT shader template | YFT v162 → FragType main fragDrawable → YDR ShaderFX | Acesso read-only e assinatura completa de shader/TextureBase, não nome do preset apenas |

## 6. Prioridade técnica para completar o Core

| Prioridade | Lacuna | Motivo |
|---|---|---|
| P0 | Corrigir semântica `container-only` do `validate` genérico | Evitar falso PASS estrutural de formatos não parseados |
| P0 | Expandir `glb validate` para separar GLB inválido, válido mas não exportável e exportável com warnings | Melhorar preflight sem duplicar o writer; o use case byte-first de export já reside no Engine |
| P1 | `glb validate` completo | Separar GLB inválido, GLB válido mas não exportável e exportável com warnings |
| P1 | `ydr inspect/validate` aprofundado | Tornar layouts, buffers, shader bindings e embedded textures auditáveis |
| P1 | YFT parser: expandir apenas com evidência para additional fragment records | Necessário para authoring/preview de veículos e props fragment; não fingir que main drawable é o fragment inteiro |
| P1 | YTYP authoring/validator aprofundado | Archetypes, MLO, bounds, rooms/portals e relationships são centrais ao Studio; writer requer contract/fixtures |
| P2 | YTD dictionary create/add/remove texture | Completa workflow de texture authoring; requer serializer/dictionary pointer layout e BC encoders com limites/testes |
| P2 | YBN geometry/topology authoring | Criar/remover children/triangles/vertices exige realloc/rebuild com invariantes de colisão; patch atual é deliberadamente limitado |
| P2 | YMF standalone authoring e round-trip | Serializer já existe, mas precisa contrato/fixtures e não somente output interno do export |
| P2 | RPF create/repack | Só iniciar se um caso real do Studio exigir; exige directory tree, compression, encryption, key policy e reopen independente |
| P3 | Importadores OBJ/FBX e mais MIME GLB | Fora da superfície atual; requer parser de verdade, scene/material semantics, fixtures e conformance, não extensão -> conversão genérica |
| P3 | YCD/YMT/YLD/YNV e formatos adicionais | Criar crates tipadas por formato, com version matrix e readers/writers específicos; não marcar suportado por RSC7 envelope |

## 7. Critérios para uma operação se tornar "stable"

Nenhuma operação muda de `experimental`/constrained para stable somente porque o parser retorna sucesso. Exigir todos os itens aplicáveis:

1. Versão/layout alvo explicitamente delimitado e documentado.
2. Parser ou writer específico do formato, sem heurística por extensão.
3. Capabilities derivadas da estrutura real do asset e dos parâmetros pedidos.
4. Testes sintéticos positivos e negativos, incluindo offsets/ranges/overflow/truncamento/casos de referência.
5. Para writes: source immutability, output non-overwrite, atomicity quando aplicável e semantic reopen comparando campos que devem ser preservados.
6. Golden reports/bytes ou invariantes determinísticas.
7. CodeWalker Core/GUI quando for um recurso Rockstar que pode ser verificado por essa ferramenta.
8. Runtime acceptance separada por target (GTA Legacy/FiveM) somente quando autorizada e efetivamente executada.
9. Schema versionado e CLI/Tauri consumindo os mesmos tipos/serviços do Core.
10. Known limitations e diagnósticos catalogados; unsupported features falham explicitamente.

## 8. Convenções de apresentação de capacidade

Use os seguintes estados públicos em reports futuros (sem quebrar imediatamente os enums existentes):
- `stable`: cobertura completa para o escopo/versão explicitados e gates aprovados.
- `constrained`: operação disponível apenas para certos layouts/índices/parametrização.
- `readOnly`: parser/inspect/validate específico, nenhum writer habilitado.
- `internal`: crate usada por outro workflow, sem operação standalone suportada.
- `containerOnly`: container parseado, sem parser semântico para o formato interno.
- `unsupported`: ausência de suporte real para formato/operação.

No estado atual, `AssetOperationAvailability` (`available`, `parameterized`, `contextRequired`, `unavailable`) continua descrevendo elegibilidade da operação por arquivo. Ele não deve ser reutilizado como status de maturidade/coverage de formato: são conceitos diferentes.

## 9. Evidência desta auditoria

- Inventário de APIs públicas e manifests das crates de formato, engine e authoring concluído em 2026-10-31.
- `cargo test -p ragelab-resource -p ragelab-meta -p ragelab-pso -p ragelab-rbf -p ragelab-rpf -p ragelab-ydr -p ragelab-ydd -p ragelab-ytd -p ragelab-ybn -p ragelab-yft -p ragelab-ymap -p ragelab-ytyp -p ragelab-ymf -p ragelab-gltf -p ragelab-authoring -p ragelab-engine`: 296 passed, 0 failed, 1 ignored. O teste ignorado `ragelab-yft::tests::parses_real_fragment_main_drawable` exige `RAGELAB_TEST_YFT` apontando para um YFT Legacy externo; não foi executado nesta rodada.
- Testes de integração CLI já executados na etapa de arquitetura: `cli_contract` 32/32 e `glb_cli` 17/17 PASS.
- Não foi realizado runtime GTA V Legacy; a etapa standalone continua WAIVED/NOT RUN. O pack FiveM tem apenas a aceitação PASS_USER_CONFIRMED anterior, fora do escopo desta auditoria de matriz.
