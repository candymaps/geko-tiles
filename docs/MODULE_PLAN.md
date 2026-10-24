# tiles: plan

Estado: esqueleto. El módulo cumple el contrato de geko (`contract-1`, [docs/MODULES.md del host](https://github.com/candymaps/geko/blob/main/docs/MODULES.md)) y pasa su suite de conformidad: declara `/tiles/{z}/{x}/{y}` y `/tiles/tilejson.json` y responde 503 a todo, porque aún no genera nada. Su publicación (`ci/modules.sh`, `publish.yml`) está lista, pero no se ha publicado ninguna versión, así que el host lo deja desactivado por defecto (`tiles = true` en `geko.toml` lo activa).

## Objetivo

Teselas vectoriales del ámbito de `geko.toml`, generadas y servidas por geko sin nada más, como el geocoder:

- **Ultraeficiente** en CPU, memoria, latencia y disco. Es el criterio que decide cada formato, compresión y estructura, siempre con medidas (AGENTS.md del host, «Optimizations»).
- **Automático:** el módulo descarga lo que necesita para el ámbito, genera su parte del bundle y la mantiene al día, con las etapas incrementales y deterministas de geko (sellos por etapa, PROJECT.md §8 del host).
- **Cualquier estilo:** el motor sirve las teselas para que el usuario las pinte como quiera. Los estilos son datos, nunca código del servidor. Cómo se configuran (un panel de control, más adelante) no entra aún en este plan.
- **Multipaís desde el principio:** una generación por país (o región) del ámbito, como los segmentos del geocoder.

Fuera, por ahora: teselas ráster, relieve, actualización por minuto (se regenera por lotes, como el resto de geko) y la configuración de estilos.

## Decidido

### Generador: Planetiler, solo en el build (2026-10-24)

[Planetiler](https://github.com/onthegomap/planetiler) (Java, Apache-2.0) genera las teselas; el servicio es Rust puro. La JVM no entra en la imagen del host ni en el proceso de geko:

- **Paquete de datos firmado del módulo** (`geko-artifact`, como los packs del geocoder): el jar del perfil y un runtime de Java reducido con `jlink` (Temurin: el perfil de Protomaps usa Planetiler 0.10.2, que pide Java 21; las versiones nuevas de Planetiler piden Java 25), con sus `LICENSE` y `NOTICE`. Lo publica la CI de este repositorio en `ghcr.io/candymaps/geko-tiles-planetiler:<versión>-<plataforma>`, el módulo lo baja en `prepare`, comprueba la firma y en `build` lo lanza como subproceso. El lanzador de un solo fichero de Planetiler se descarta: es experimental y no fija nuestra versión del runtime.
- **El pico de memoria del build se queda en el proceso de Java,** no en el de geko (MODULES.md del host, «Adaptador»). `-Xmx`, hilos y `--storage=mmap` salen de `geko-tiles.toml`.
- **Las descargas son del `Downloader` del host,** como en el geocoder: el módulo decide qué URL necesita y dónde dejarla, y las baja con el `Downloader` que recibe por la SDK, sin cliente HTTP propio ni `--download` de Planetiler. Así valen `offline`, la caché `raw/`, `accept_licenses` y las licencias del manifiesto. Los extractos de Geofabrik son las mismas URLs que las del geocoder, para compartirse en `raw/`; además, los polígonos de agua (osmdata) y Natural Earth. Planetiler recibe las rutas locales.

### Esquema: Protomaps Basemap (2026-10-24)

El perfil de Planetiler de [Protomaps](https://github.com/protomaps/basemaps): mantenido por sus autores para Planetiler, código BSD-3, diseño de sus estilos CC0, con fuentes y sprites propios. Shortbread en Planetiler es solo un YAML de ejemplo, y OpenMapTiles obliga a atribuir su esquema (CC-BY 4.0). Un estilo solo puede pintar lo que llevan las teselas: pintar datos que el esquema no lleva es cambiar el perfil, un cambio del módulo.

### Formato en el bundle: propio, para servir

Planetiler deja un PMTiles por partición, que es solo un formato de trabajo. Una etapa en Rust los fusiona y los convierte a un formato pensado para servir con una sola búsqueda:

- `hot/index.bin` (fijado en RAM): los `tile_id` de Hilbert ordenados (`u64`) y, en paralelo, su posición y tamaño empaquetados (`u64`). Búsqueda binaria; si se mide que compensa, disposición de Eytzinger.
- `tiles.bin` (mapeado, sin fijar): las teselas comprimidas, una tras otra. Las idénticas (mar, tierra vacía) se guardan una vez y varias entradas del índice apuntan a ellas.
- Las teselas de frontera entre particiones se fusionan juntando las capas del mismo nombre, con las claves y los valores reindexados. Determinista.
- `geko tiles export` dará un PMTiles para quien lo quiera.

### Servicio

- `/tiles/{z}/{x}/{y}`: analizar `z/x/y`, calcular el id de Hilbert, buscar en el índice, copiar la porción al `Response`. Sin descomprimir ni calcular nada, y sin otra asignación de memoria que la del cuerpo. `pool: false`. 204 si no hay tesela (MapLibre la toma por vacía).
- `/tiles/tilejson.json` con la atribución y las teselas en `/tiles/{z}/{x}/{y}?v=<id del bundle>`: las teselas van con `Cache-Control: public, max-age=31536000, immutable`, así que ni el cliente ni una CDN vuelven a preguntar, sin ETag ni 304. Un bundle nuevo cambia la URL.
- Los estilos, fuentes y sprites se sirven como bytes estáticos del bundle, precomprimidos: el motor no sabe qué estilo es. De entrada, los de Protomaps con sus URLs apuntando a las propias.

## Por decidir (con las medidas de la prueba, paso 1)

Cerrados con Baleares («Medidas»): la compresión (el gzip de Planetiler; brotli solo gana un 5 %), el zoom máximo (14) y el determinismo (sí, los mismos bytes).

1. **MLT frente a MVT** (`--tile-format=mlt`): un 10 % menos con Baleares; depende de que la versión de MapLibre lo decodifique.
2. **Las entradas globales** (agua, tierra, Natural Earth, landcover, qrank): paquetes de datos recortados por país, publicados por este repositorio, o el `Downloader` del host con las globales. Con Baleares son el 77 % del tiempo del build y el 98 % de la descarga.
3. **Recortar las teselas al estilo:** quitar capas, atributos y zooms que ningún estilo usa. Ahorra disco y red, pero entonces un cambio de estilo regenera teselas.
4. Si Planetiler toma varios extractos de OSM a la vez, o si cada partición va por separado y se fusiona (lo propuesto arriba).
5. **España (aplazado, 2026-10-24):** tiempo, memoria y tamaño del build, y tamaño del índice. Lo decidido con Baleares (compresión, zoom máximo, determinismo, formato del índice) no depende del tamaño del ámbito, así que se sigue sin medir España y se mide cuando el build esté hecho (paso 4), por si hace falta optimizar. Mientras, opciones prudentes por defecto: `--storage=mmap` y `-Xmx` configurable en `geko-tiles.toml`.

## Pasos

1. **Prueba con medidas** (Baleares congelada del golden del geocoder; hecha. España, aplazada: «Por decidir», 5): el perfil de Protomaps con Planetiler en Docker. Tiempo y RAM del build; tamaño con MVT y MLT, con gzip, zopfli y brotli, y con zoom máximo 13 y 14; cuántas teselas se repiten; si dos ejecuciones dan los mismos bytes. Cierra los puntos de «Por decidir». Los resultados van a «Medidas», abajo.
2. **Puesta al día del repositorio:** la SDK en la etiqueta `sdk-1`, README y AGENTS.md como los del geocoder, el secreto `GEKO_PACK_SIGNING_KEY` y la decisión de licencias por escrito (abajo).
3. **Paquete de Planetiler** en la CI: el jar del perfil y el runtime de `jlink`, firmado y publicado.
4. **Build:** `plan`, `fetch`, `prepare` y `build`, con una etapa por partición y la de fusión y conversión, cada una con su sello (la clave lleva las versiones de Planetiler y del perfil, y sus opciones). La atribución ODbL en el manifiesto.
5. **Servicio:** el índice, las rutas, `tilejson.json` con las URL versionadas y las métricas (`geko_tiles_*`). Latencia (p50 y p99) y caudal con 16 a la vez, anotados en «Optimizations».
6. **Estilos, fuentes y sprites** de Protomaps servidos desde el bundle; la demo del host con las teselas propias.
7. **Comprobaciones:** un golden de teselas de Baleares (unas cuantas teselas de referencia comparadas byte a byte con datos congelados, como el del geocoder), pruebas del índice y de la fusión con teselas sintéticas, y la conformidad.
8. **Publicación:** la primera versión publicada; `tiles` entra en `ci/modules` del host y vuelve a estar activo por defecto.
9. **Después, según las medidas:** brotli, MLT, el recorte al estilo y, con el panel de control, estilos propios del usuario.

## Licencias

- **El código** es AGPL-3.0-only, como el resto de geko. Planetiler corre en un proceso aparte y solo intercambia ficheros: es agregación (AGPLv3 §5), y ninguna licencia se contagia en ningún sentido.
- **El paquete de Planetiler** que se redistribuye lleva Apache-2.0 (Planetiler), **LGPL** (GeoTools: `gt-shapefile`, `gt-epsg-hsql`), EDL (JTS) y, el runtime, **GPLv2 con Classpath Exception**. Se cumple acompañando las licencias, los `NOTICE` y el acceso al código de las partes LGPL y GPL. `deny.toml` pide una decisión antes de un copyleft: esta es esa decisión, para un paquete aparte que no se enlaza con el código de geko, pendiente de revisar con el punto 26 del host (un abogado antes de abrir el servicio).
- **Las teselas** son una obra producida (*Produced Work*) a partir de OSM (ODbL): llevan «© OpenStreetMap contributors» en `tilejson.json`, en `/v1/attribution` y en el estilo. Natural Earth es de dominio público; los polígonos de agua, datos de OSM.

## Medidas

### Baleares, 2026-10-24 (paso 1)

Perfil de Protomaps (`42ffaaa`, Planetiler 0.10.2) en `eclipse-temurin:21-jre`, en WSL (16 hilos, 15 GB), sobre el extracto congelado del golden del geocoder (`islas-baleares-latest.osm.pbf`, 47 MB). qrank vacío: `qrank.toolforge.org` daba 503 («no healthy upstream»), y solo cambia el orden de lugares y POIs, no los tamaños.

**Entradas:** 2,3 GB, el 98 % global: polígonos de agua (932 MB) y de tierra (953 MB) de osmdata, Natural Earth (446 MB), landcover de Daylight (36 MB), fuentes de pgf-encoding (0,8 MB) y qrank. Planetiler baja además `tile_weights.tsv.gz` de GitHub por su cuenta.

**Build** (pico del contenedor, con la caché de páginas de los ficheros mapeados, así que por encima del de Java):

| Zoom máx. | Tiempo | CPU | Pico | PMTiles | Teselas (únicas) |
|---|---|---|---|---|---|
| 13 | 1 min 49 s | 5 min 54 s | 3,0 GB | 12,2 MB | 10.671 (2.360) |
| 14 | 1 min 58 s | 7 min 49 s | 3,8 GB | 25,0 MB | 41.757 (6.135) |
| 14, MLT | 1 min 57 s | 8 min 3 s | 3,7 GB | 22,4 MB | 41.757 (6.135) |
| 15 (el del perfil) | 1 min 59 s | 8 min 13 s | 3,8 GB | 52,8 MB | 165.392 (16.799) |

- **El tiempo es de las entradas globales, no de OSM:** con z14, Natural Earth 71 s (lee el gpkg mundial con un hilo), agua y tierra 20 s, las dos pasadas de OSM 15 s y la escritura de teselas 10 s.
- **Determinista:** dos ejecuciones con z14 dan los mismos bytes (sha256 `9a902eab…`).
- **Cada zoom duplica el tamaño:** z15 es el 54 % del total con z15; z14, el 51 % con z14.
- **Repetidas:** el 85 % de las teselas direccionadas de z14 repiten contenido (sobre todo mar); en run-lengths quedan 8.881 entradas, unos 180 KB de índice a 20 B por entrada.
- **Tamaño de las teselas** (z14, gzip): mediana 225 B, p90 8,4 KB, p99 35 KB, máximo 93 KB.

**Recompresión** de las 6.135 teselas únicas de z14 (25,0 MB tal como las deja Planetiler, en gzip): gzip-9 25,0 MB (igual), zopfli 24,5 MB (−1,6 %), brotli-11 23,7 MB (**−4,9 %**). Las teselas son pequeñas, y a brotli no le da para más. **MLT** baja un 10 % (22,4 MB, también en gzip).

### Conclusiones

1. **Compresión:** se queda el gzip de Planetiler. Brotli no compensa un cambio del contrato por un 5 %, y zopfli tampoco por un 1,6 %.
2. **MLT:** −10 % de disco y de red, sin coste en el build. Se adopta si MapLibre GL JS de la demo lo decodifica; si no, MVT.
3. **Zoom máximo 14:** la mitad que z15, con sobrezoom en el cliente.
4. **Las entradas globales son el problema del build**, en tiempo (el 77 %), en descarga y en disco, y algunas dependen de servicios ajenos e inestables (qrank). Propuesta: paquetes de datos firmados del módulo, por país o región, con esas fuentes ya recortadas (agua, tierra, Natural Earth, landcover, qrank), que publica la CI de este repositorio, como los packs del geocoder. Falta ver si los polígonos de tierra hacen falta (la capa `earth` podría salir de los de agua).
5. **Índice propio:** con run-lengths, unos 180 KB para Baleares; falta medirlo con España (aplazado al paso 4).
