# tiles: plan

Estado: esqueleto. El módulo cumple el contrato de geko (`contract-1`, [docs/MODULES.md del host](https://github.com/candymaps/geko/blob/main/docs/MODULES.md)) y pasa su suite de conformidad: declara `/tiles/{z}/{x}/{y}` y `/tiles/tilejson.json` y responde 503 a todo, porque aún no genera nada. Su publicación (`ci/modules.sh`, `publish.yml`) está lista, pero no se ha publicado ninguna versión, así que el host lo deja desactivado por defecto (`tiles = true` en `geko.toml` lo activa).

## Objetivo

Teselas vectoriales del ámbito de `geko.toml`, generadas y servidas por geko sin nada más, como el geocoder:

- **Ultraeficiente** en CPU, memoria, latencia y disco. Es el criterio que decide cada formato, compresión y estructura, siempre con medidas (AGENTS.md del host, «Optimizations»).
- **Automático:** el módulo descarga lo que necesita para el ámbito, genera su parte del bundle y la mantiene al día, con las etapas incrementales y deterministas de geko (sellos por etapa, PROJECT.md §8 del host).
- **Cualquier estilo:** el motor sirve las teselas para que el usuario las pinte como quiera. Los estilos son datos, nunca código del servidor. Cómo se configuran (un panel de control, más adelante) no entra aún en este plan.
- **Multipaís desde el principio:** cualquier combinación de países y regiones del ámbito, en una sola generación («Varios extractos»).

Fuera, por ahora: teselas ráster, relieve, actualización por minuto (se regenera por lotes, como el resto de geko) y la configuración de estilos.

## Decidido

### Generador: Planetiler, solo en el build (2026-10-24)

[Planetiler](https://github.com/onthegomap/planetiler) (Java, Apache-2.0) genera las teselas; el servicio es Rust puro. La JVM no entra en la imagen del host ni en el proceso de geko:

- **Sin paquete propio de Planetiler (2026-10-24):** el módulo baja con el `Downloader` del host, de su origen y con el sha256 fijado en el código, el `planetiler.jar` oficial de la release de Planetiler (0.10.2, 93 MB, con todas sus dependencias) y el JRE de Temurin para su plataforma (Adoptium; Java 21, el que pide Planetiler 0.10.2; las versiones nuevas piden Java 25). El perfil de Protomaps, con nuestros parches, es un jar de 375 KB que solo depende de `planetiler-core`: lo compila la CI de este repositorio y va dentro del propio paquete del módulo, que ya se publica firmado. En `build` se lanza como subproceso: `java -cp planetiler.jar:protomaps-basemap.jar com.protomaps.basemap.Basemap …`. Comprobado con La Rioja: las mismas teselas, byte a byte, que con el jar del perfil con dependencias; solo cambia la fecha de compilación de Planetiler en los metadatos. El lanzador de un solo fichero de Planetiler se descarta: es experimental y no fija nuestra versión del runtime.
- **El pico de memoria del build se queda en el proceso de Java,** no en el de geko (MODULES.md del host, «Adaptador»). `-Xmx`, hilos y `--storage=mmap` salen de `geko-tiles.toml`.
- **Las descargas son del `Downloader` del host,** como en el geocoder: el módulo decide qué URL necesita y dónde dejarla, y las baja con el `Downloader` que recibe por la SDK, sin cliente HTTP propio ni `--download` de Planetiler. Así valen `offline`, la caché `raw/`, `accept_licenses` y las licencias del manifiesto. Los extractos de Geofabrik son las mismas URLs que las del geocoder, para compartirse en `raw/`; además, los polígonos de agua (osmdata) y Natural Earth. Planetiler recibe las rutas locales.

### Esquema: Protomaps Basemap (2026-10-24)

El perfil de Planetiler de [Protomaps](https://github.com/protomaps/basemaps): mantenido por sus autores para Planetiler, código BSD-3, diseño de sus estilos CC0, con fuentes y sprites propios. Shortbread en Planetiler es solo un YAML de ejemplo, y OpenMapTiles obliga a atribuir su esquema (CC-BY 4.0). Un estilo solo puede pintar lo que llevan las teselas: pintar datos que el esquema no lleva es cambiar el perfil, un cambio del módulo.

### Formato en el bundle: propio, para servir

Planetiler deja un PMTiles del ámbito, que es solo un formato de trabajo. Una etapa en Rust lo convierte a un formato pensado para servir con una sola búsqueda:

- `hot/index.bin` (fijado en RAM): los `tile_id` de Hilbert ordenados (`u64`) y, en paralelo, su posición y tamaño empaquetados (`u64`). Búsqueda binaria; si se mide que compensa, disposición de Eytzinger.
- `tiles.bin` (mapeado, sin fijar): las teselas comprimidas, una tras otra. Las idénticas (mar, tierra vacía) se guardan una vez y varias entradas del índice apuntan a ellas.
- `geko tiles export` dará un PMTiles para quien lo quiera.

### Servicio

- `/tiles/{z}/{x}/{y}`: analizar `z/x/y`, calcular el id de Hilbert, buscar en el índice, copiar la porción al `Response`. Sin descomprimir ni calcular nada, y sin otra asignación de memoria que la del cuerpo. `pool: false`. 204 si no hay tesela (MapLibre la toma por vacía).
- `/tiles/tilejson.json` con la atribución y las teselas en `/tiles/{z}/{x}/{y}?v=<id del bundle>`: las teselas van con `Cache-Control: public, max-age=31536000, immutable`, así que ni el cliente ni una CDN vuelven a preguntar, sin ETag ni 304. Un bundle nuevo cambia la URL.
- Los estilos, fuentes y sprites se sirven como bytes estáticos del bundle, precomprimidos: el motor no sabe qué estilo es. De entrada, los de Protomaps con sus URLs apuntando a las propias.

### Formato de las teselas: MVT, y MLT como opción (2026-10-24)

Las teselas son para cualquiera que use el motor, con el cliente que quiera (MapLibre, OpenLayers, QGIS, apps móviles, versiones antiguas…). MLT solo lo lee MapLibre GL JS desde la 5.12.0 (`encoding: "mlt"` en la fuente) y su especificación aún evoluciona, así que **MVT por defecto**. MLT, un 10 % menor con Baleares, queda como opción de `geko-tiles.toml` (`format = "mvt" | "mlt"`) para quien controle sus clientes: Planetiler genera los dos con `--tile_format` y el servicio solo copia bytes. Falta ver cómo lo anuncia `tilejson.json` a los clientes.

### Varios extractos: fusionados antes, una sola ejecución (2026-10-24)

Medido con La Rioja y Navarra («Medidas»): una ejecución por extracto paga varias veces las entradas globales (un 77 % más de tiempo) y deja teselas compartidas con un 6 % de objetos repetidos, que una fusión de teselas no deja igual que una sola ejecución. Así que:

- **Una etapa en Rust fusiona los extractos del ámbito en un solo PBF:** los lee en orden (nodos, vías y relaciones, cada tipo por id, como los publica Geofabrik), se queda con una copia de cada objeto repetido y escribe el resultado. Determinista y con su sello: solo se repite si cambia algún extracto. Con un solo extracto no hace nada. Para escribir el PBF: evaluar [pbfhogg](https://docs.rs/crate/pbfhogg/0.5.0/source/README.md) (MIT o Apache-2.0, lee y escribe; joven, 0.5) o escribirlo nosotros (protobuf y zlib), según cuál dé un resultado determinista y pase `cargo deny`.
- **Una sola ejecución de Planetiler por ámbito,** con `--bounds=world` («Entradas globales»): el PBF fusionado no lleva la caja en el encabezado (sin `--bounds`, Planetiler la saca de los nodos y genera de más: 47.365 teselas en vez de 14.076), y las teselas de fuera del ámbito las quita la etapa de conversión.
- **Sin fusión de teselas:** cada tesela sale de una sola ejecución.
- A cambio, si cambia un extracto se genera de nuevo todo el ámbito. En la práctica da igual: Geofabrik publica todos los extractos cada día.

### Entradas globales: sin paquetes publicados (2026-10-24)

Nada de paquetes de datos recortados publicados por este repositorio. Cada instalación baja lo que necesita con el `Downloader` del host y lo recorta en local:

- **Natural Earth, por capas:** el perfil usa solo 11 capas del gpkg mundial (446 MB): `ne_50m_land`, `ne_10m_land`, `ne_50m_ocean`, `ne_10m_ocean`, `ne_50m_lakes`, `ne_10m_lakes`, `ne_10m_admin_0_boundary_lines_land`, `ne_10m_admin_0_boundary_lines_map_units`, `ne_10m_admin_0_boundary_lines_disputed_areas`, `ne_10m_admin_1_states_provinces_lines` y `ne_10m_glaciated_areas`. Se bajan solo esas, en shapefile (unos 72 MB sin comprimir), del repositorio de Natural Earth en la etiqueta de la versión del gpkg (`raw.githubusercontent.com/nvkelso/natural-earth-vector/v5.1.2/…`; las de naciscdn siguen en la 5.1.1). El perfil las lee como la fuente `ne` con `addShapefileGlobSource("ne", …, "*.shp")`, y lee las claves también en mayúsculas, como vienen en los shapefiles. Dos parches al perfil: esa fuente, y tomar una cadena vacía como valor ausente (`getNeString` de `Boundaries`: el shapefile trae `brk_a3 = ""` donde el gpkg trae nulo). Con Baleares, Natural Earth pasa de 71 s a 6 s («Medidas»).
- **Planetiler con `--bounds=world`; el ámbito lo ponen el recorte y la conversión.** El lector de shapefiles de Planetiler solo pide los objetos que tocan `--bounds` (el de gpkg lee todo si la tabla no tiene índice espacial, como el de Natural Earth): con los límites del ámbito, en z5 desaparecían Cerdeña y otras islas vecinas de Baleares, y en su lugar salía mar. Con el mundo entero, Natural Earth sale completo en z0–z5 (1.365 teselas de contexto, de todo el mundo), y el resto lo limitan el recorte de osmdata (abajo) y la etapa de conversión, que solo guarda, de z6 en adelante, las teselas que tocan el ámbito.
- **Polígonos de agua y tierra (osmdata):** solo existen para todo el planeta, en zips de casi 1 GB cada uno, así que se bajan enteros. Una etapa en Rust, con su sello, se queda con los registros que tocan el ámbito (vienen en una rejilla y cada registro del shapefile lleva su caja) y Planetiler recibe ficheros pequeños. Después se borra el original y se conserva su ficha (`Downloader::keeps_removed`): solo se vuelve a bajar si osmdata publica otro.
- **qrank, opcional:** si no responde, el build sigue sin él y lo avisa en el log (solo cambia el orden de lugares y POIs).
- **Landcover de Daylight** (36 MB) y **pgf-encoding** (0,8 MB), tal cual.

Más adelante, si hace falta: quitar los polígonos de tierra (la capa `earth` podría salir de los de agua) o sacar el mar de las costas del propio extracto de OSM (difícil: el extracto corta las costas en su borde).

## Por decidir (con las medidas de la prueba, paso 1)

Cerrados con Baleares («Medidas»): la compresión (el gzip de Planetiler; brotli solo gana un 5 %), el zoom máximo (14) y el determinismo (sí, los mismos bytes).

1. ~~MLT frente a MVT~~: decidido, «Formato de las teselas».
2. ~~Las entradas globales~~: decidido, «Entradas globales».
3. **Recortar las teselas al estilo:** quitar capas, atributos y zooms que ningún estilo usa. Ahorra disco y red, pero entonces un cambio de estilo regenera teselas.
4. ~~Varios extractos~~: decidido, «Varios extractos».
5. **España (aplazado, 2026-10-24):** tiempo, memoria y tamaño del build, y tamaño del índice. Lo decidido con Baleares (compresión, zoom máximo, determinismo, formato del índice) no depende del tamaño del ámbito, así que se sigue sin medir España y se mide cuando el build esté hecho (paso 4), por si hace falta optimizar. Mientras, opciones prudentes por defecto: `--storage=mmap` y `-Xmx` configurable en `geko-tiles.toml`.

## Pasos

1. **Prueba con medidas** (Baleares congelada del golden del geocoder; hecha. España, aplazada: «Por decidir», 5): el perfil de Protomaps con Planetiler en Docker. Tiempo y RAM del build; tamaño con MVT y MLT, con gzip, zopfli y brotli, y con zoom máximo 13 y 14; cuántas teselas se repiten; si dos ejecuciones dan los mismos bytes. Cierra los puntos de «Por decidir». Los resultados van a «Medidas», abajo.
2. **Puesta al día del repositorio** (hecho, 2026-10-24): la SDK en la etiqueta `sdk-1`, README y AGENTS.md como los del geocoder, el secreto `GEKO_PACK_SIGNING_KEY` (firma el módulo al publicarlo) y la decisión de licencias por escrito (abajo).
3. **Perfil y runtime:** el perfil de Protomaps con nuestros parches («Entradas globales»: Natural Earth por capas con `addShapefileGlobSource`, la cadena vacía como valor ausente y los shapefiles de osmdata ya recortados), compilado en la CI y metido en el paquete del módulo; la descarga de `planetiler.jar` y del JRE de Temurin, con sus sha256, por el `Downloader`.
4. **Build:** `plan`, `fetch`, `prepare` y `build`, con las etapas de recorte de las entradas globales, fusión de los extractos, Planetiler (`--bounds=world`) y conversión al formato del bundle (que solo guarda, de z6 en adelante, las teselas que tocan el ámbito), cada una con su sello (la clave lleva las versiones de Planetiler y del perfil, y sus opciones). La atribución ODbL en el manifiesto.
5. **Servicio:** el índice, las rutas, `tilejson.json` con las URL versionadas y las métricas (`geko_tiles_*`). Latencia (p50 y p99) y caudal con 16 a la vez, anotados en «Optimizations».
6. **Estilos, fuentes y sprites** de Protomaps servidos desde el bundle; la demo del host con las teselas propias.
7. **Comprobaciones:** un golden de teselas de Baleares (unas cuantas teselas de referencia comparadas byte a byte con datos congelados, como el del geocoder), pruebas del índice con teselas sintéticas y de la fusión de extractos con PBF sintéticos (objetos repetidos, orden, determinismo), y la conformidad.
8. **Publicación:** la primera versión publicada; `tiles` entra en `ci/modules` del host y vuelve a estar activo por defecto.
9. **Después, según las medidas:** brotli, MLT, el recorte al estilo y, con el panel de control, estilos propios del usuario.

## Licencias

- **El código** es AGPL-3.0-only, como el resto de geko. Planetiler corre en un proceso aparte y solo intercambia ficheros: es agregación (AGPLv3 §5), y ninguna licencia se contagia en ningún sentido.
- **Planetiler y el JRE no se redistribuyen:** cada instalación los baja de su origen (la release de Planetiler y Adoptium), como las fuentes de datos. Lo que lleva Planetiler (Apache-2.0, **LGPL** de GeoTools: `gt-shapefile`, `gt-epsg-hsql`, EDL de JTS) y el runtime (**GPLv2 con Classpath Exception**) llega con sus propias licencias, sin obligaciones de redistribución para nosotros. Solo se redistribuye el perfil de Protomaps (BSD-3) con nuestros parches, dentro del paquete del módulo, con su `LICENSE`. Pendiente de revisar con el punto 26 del host (un abogado antes de abrir el servicio).
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
2. **MLT:** −10 % de disco y de red, sin coste en el build. Queda como opción; por defecto, MVT («Formato de las teselas»).
3. **Zoom máximo 14:** la mitad que z15, con sobrezoom en el cliente.
4. **Las entradas globales son el problema del build**, en tiempo (el 77 %), en descarga y en disco, y algunas dependen de servicios ajenos e inestables (qrank). Se resuelve sin paquetes publicados: Natural Earth por capas, osmdata recortado en local y qrank opcional («Entradas globales»).
5. **Índice propio:** con run-lengths, unos 180 KB para Baleares; falta medirlo con España (aplazado al paso 4).

### Varios extractos: La Rioja y Navarra, 2026-10-24

Dos regiones vecinas (`la-rioja`, 14 MB, y `navarra`, 38 MB, de Geofabrik), z14, mismo entorno que Baleares. Fusionadas con `osmium merge` (51 MB, quita los objetos repetidos) en una sola ejecución, o una ejecución por región, sin recorte y con `--clip` al polígono de cada región (de `index-v1.json` de Geofabrik).

| Ejecución | Tiempo | PMTiles | Teselas |
|---|---|---|---|
| Fusionada (`--bounds` de las dos) | 2 min 2 s | 50,2 MB | 14.076 |
| La Rioja + Navarra por separado | 1 min 44 s + 1 min 53 s = 3 min 37 s | 12,2 + 38,9 MB | 4.164 + 10.534 |
| Con `--clip` | 1 min 43 s + 1 min 54 s | 11,8 + 38,1 MB | 2.937 + 5.773 |

- **Por separado cuesta un 77 % más:** cada ejecución paga entera las entradas globales (Natural Earth 67 s, agua y tierra 20 s), y la parte de OSM es pequeña.
- **Teselas compartidas:** sin recorte, 2.373 (el 19 %), entre ellas todas las de z0 a z7; con `--clip`, 634.
- **Objetos repetidos en las compartidas:** sin recorte, unos 10.700 objetos con el mismo id en las dos ejecuciones (el 6 %): el mar y la tierra (2.377 de 5.365), los usos del suelo (5.180), los lugares (1.002), las vías (922)… Los extractos de Geofabrik llevan enteras las vías que cruzan su borde. Juntar las capas los duplicaría; quitarlos por id se acerca a la ejecución fusionada (usos del suelo 79.432 frente a 79.426; lugares 35.932 frente a 35.931), pero no da lo mismo: las vías y los límites salen partidos de otra forma (23.954 frente a 23.319; 2.096 frente a 1.982), porque Planetiler une las líneas al reducir el zoom. `--clip` no lo evita (sigue habiendo 4.597 usos del suelo repetidos, por el margen del recorte y lo que cruza el borde) y además quita lo que el extracto trae de fuera del polígono.
- **`osmium merge` pierde la caja del encabezado:** sin `--bounds`, Planetiler la saca de los nodos y genera de más (47.365 teselas). Se resuelve con `--bounds=world` y quitando en la conversión las teselas de fuera del ámbito (abajo).
- **El jar oficial de Planetiler da lo mismo:** `planetiler.jar` de la release 0.10.2 con el jar del perfil sin dependencias genera las mismas 4.164 teselas de La Rioja, byte a byte («Generador»).

### Natural Earth por capas y `--bounds=world`: Baleares, 2026-10-24

El `planetiler.jar` oficial con el perfil parcheado («Entradas globales»), z14, contra la ejecución original de Baleares (gpkg de Natural Earth, `--bounds` del extracto: 2 min, 41.757 teselas).

| Ejecución | Tiempo | Natural Earth | Diferencias con la original |
|---|---|---|---|
| Capas de Natural Earth, `--bounds` del extracto | 1 min | 6 s (antes 71 s) | 6 teselas, z0–z5: en z5 falta Cerdeña |
| Capas, `--bounds=world` y osmdata recortado al ámbito | 58 s | 8 s | Ninguna de z8 a z14; z0–z7 con todo el mundo |

- **Cerdeña:** en la tesela 5/16/12 la ejecución por capas perdía un 4,4 % de la tierra (Cerdeña, Sant'Antioco, San Pietro, Asinara y Kerkennah), aunque los datos de `ne_10m_land` son idénticos en el gpkg y en el shapefile (8 polígonos, 1.769 vértices, misma área en esa tesela). La causa es el filtro por `--bounds` del lector de shapefiles. Con `--bounds=world` vuelve igual que en la original (misma área, 16 partes).
- **Recorte de osmdata** (por la caja de cada registro, como hará la etapa en Rust; en Python para la prueba): agua, 4 de 14.509 registros en 28 s; tierra, 457 de 848.247 en 41 s. Quedan 4,3 MB. Planetiler los lee en 1 s (antes 20 s).
- **Teselas de más:** con el mundo entero salen 102.437 teselas (47,1 MB) en vez de 41.757 (28,3 MB): z0–z7 de todo el mundo (Natural Earth y landcover) y, de z8 en adelante, las que tocan las celdas de osmdata que asoman fuera del ámbito. Todas las teselas originales de z8 a z14 están, byte a byte. La conversión solo guarda, de z6 en adelante, las que tocan el ámbito.
