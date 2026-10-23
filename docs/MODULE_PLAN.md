# tiles: plan

Estado: esqueleto. El módulo cumple el contrato de geko (`contract-1`, [docs/MODULES.md del host](https://github.com/candymaps/geko/blob/main/docs/MODULES.md)) y pasa su suite de conformidad: declara `/tiles/{z}/{x}/{y}` y `/tiles/tilejson.json` y responde 503 a todo, porque aún no genera nada. Su publicación (`ci/modules.sh`, `publish.yml`) está lista, pero no se ha publicado ninguna versión, así que el host lo deja desactivado por defecto (`tiles = true` en `geko.toml` lo activa).

## Objetivo

Teselas vectoriales del ámbito de `geko.toml`, generadas y servidas por geko sin nada más, como el geocoder:

- **Automático:** el módulo descarga lo que necesita para el ámbito, genera su parte del bundle y la mantiene al día, con las etapas incrementales y deterministas de geko (sellos por etapa, PROJECT.md §8 del host).
- **Servido por geko:** `/tiles/{z}/{x}/{y}` (MVT), `/tiles/tilejson.json` y un estilo de MapLibre, con las claves de API, CORS y límites del host. La demo de `/` las usa en vez de teselas externas.
- **Rápido y ligero:** una tesela se lee del bundle mapeado en memoria, sin generarla al pedirla.
- **Multipaís desde el principio:** una parte del bundle por país (o región) del ámbito, como los segmentos del geocoder.

Fuera, por ahora: teselas ráster, relieve, actualización por minuto (se regenera por lotes, como el resto de geko).

## Por decidir

### 1. Generador

Es lo que más pesa: decide el lenguaje, la memoria y el tiempo de generación con España.

| Opción | A favor | En contra |
|---|---|---|
| **Planetiler** (Java) | Muy rápido y maduro; perfiles de OpenMapTiles y Shortbread ya hechos | Necesita una JVM: dentro de la imagen del host (que no lleva módulos) o descargada como «fuente». Rompe que el módulo sea una biblioteca que carga geko, y sus etapas no tendrían los sellos de geko |
| **tilemaker** (C++) | Maduro, perfiles en Lua, sin JVM | Otro binario que descargar y ejecutar fuera del proceso, con su propia firma; la misma pega con los sellos |
| **Propio en Rust**, dentro del módulo | Encaja con todo: biblioteca, sellos por etapa, determinismo, `osmpbf` y `geko-geom` como en el geocoder, una sola compilación | Trabajo: simplificación por zoom, recorte, unión de polígonos, codificación MVT. Se mitiga empezando con pocas capas |

Recomendación: **propio en Rust**, empezando por pocas capas (abajo, paso 3), y medir con España antes de ampliarlo. Si el tiempo o la calidad no salen, Planetiler como proceso aparte es la salida.

### 2. Esquema de las teselas

Decide con qué estilos de mapa funcionan.

| Opción | Licencia del esquema | Notas |
|---|---|---|
| **Shortbread** | CC0 | Sencillo, pensado para OSM, con estilos de MapLibre abiertos (VersaTiles) |
| OpenMapTiles | CC-BY 4.0 (exige atribución también del esquema) | El más usado, muchos estilos; más capas y reglas que implementar |
| Protomaps Basemap | BSD-3 / CC0 | Bien documentado; pensado para su propio generador |

Recomendación: **Shortbread**, por licencia y por tamaño.

### 3. Formato en el bundle

**PMTiles**, un fichero por país (o región) del ámbito: un índice de directorios y las teselas comprimidas, leídos con `mmap` como los índices del geocoder, sin base de datos. MBTiles (SQLite) se descarta: añade SQLite y no se mapea. Falta decidir cómo se sirven varios países a la vez: un PMTiles por país y la tesela del primero que la tenga, o uno solo para todo el ámbito (más simple de servir, pero se regenera entero al cambiar un país).

### 4. Datos

- **Fuente:** OpenStreetMap, los mismos extractos de Geofabrik por país y región que el geocoder. Hay que comprobar que el `Downloader` del host los baja una sola vez para los dos módulos (misma URL en `raw/`) y qué pasa con ellos si uno de los dos se desactiva (punto 48 del host).
- **Datos por país propios** (aplazado en el host desde el 2026-10-21): de entrada, ninguno. Lo que pueda ser de un país (nombres en el idioma local, límites) sale de OSM. Si hace falta algo, irá como paquete de datos firmado del módulo, como los packs del geocoder.
- **Agua y costa:** los polígonos de costa de OSM (osmdata.openstreetmap.de) son globales y grandes; recortarlos al ámbito.

### 5. Estilo y recursos

El estilo de MapLibre, las fuentes (glyphs) y los iconos (sprites): servidos por el módulo bajo `/tiles/`, para no depender de nadie, o de una CDN externa. Recomendación: servirlos, con un estilo Shortbread abierto adaptado.

### 6. Zooms

Del 0 al 14, con sobrezoom en el cliente a partir del 14. Los zooms bajos de un solo país salen casi vacíos: se decide si se generan.

## Pasos (propuesta)

1. **Puesta al día del repositorio:** la SDK en la etiqueta `sdk-1`, README y AGENTS.md como los del geocoder, el secreto `GEKO_PACK_SIGNING_KEY` y, cuando sea público, las reglas de protección de `main` y `dev`.
2. **Decidir** los puntos 1 a 3.
3. **Generación mínima:** carreteras, agua, límites administrativos y lugares (`place=*`), de Baleares, en un PMTiles del bundle, con sus etapas y sellos. Medir tiempo, memoria y tamaño con Baleares y con España.
4. **Servicio:** `/tiles/{z}/{x}/{y}` desde el PMTiles mapeado, `/tiles/tilejson.json` con la atribución, cabeceras de caché y `Content-Encoding`. Medir la latencia por tesela y el caudal con 16 a la vez.
5. **Estilo y demo:** el estilo y sus recursos; la demo del host con las teselas propias.
6. **Resto de capas** de Shortbread: edificios, usos del suelo, POIs, transporte, etiquetas de calles.
7. **Publicación:** la primera versión publicada; `tiles` entra en `ci/modules` del host y vuelve a estar activo por defecto.
8. **Comprobaciones:** un golden de teselas (unas cuantas teselas de referencia de Baleares, comparadas byte a byte con datos congelados, como el golden del geocoder) y la variante `instructions` para la codificación.

## Licencias

Las teselas son una obra producida (*Produced Work*) a partir de OSM: llevan la atribución «© OpenStreetMap contributors» en `tilejson.json`, en `/v1/attribution` y en el estilo. Si se usa OpenMapTiles, también su propia atribución. El código es AGPL-3.0-only, como el resto de geko.
