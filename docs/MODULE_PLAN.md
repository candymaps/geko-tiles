# tiles: plan

Estado: el build, el servicio, los estilos y las comprobaciones están hechos (pasos 4 a 7); falta la publicación (paso 8). El módulo cumple el contrato de geko (`contract-1`, [docs/MODULES.md del host](https://github.com/candymaps/geko/blob/main/docs/MODULES.md)) y pasa su suite de conformidad: descarga lo que necesita el ámbito, genera las teselas con Planetiler, deja su parte del bundle y la sirve en `/tiles/{z}/{x}/{y}` y `/tiles/tilejson.json`, con los estilos de Protomaps, sus fuentes y sus sprites. Su publicación (`ci/modules.sh`, `publish.yml`) está lista, pero no se ha publicado ninguna versión, así que el host lo deja desactivado por defecto (`tiles = true` en `geko.toml` lo activa).

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

- **Sin paquete propio de Planetiler (2026-10-24):** el módulo baja con el `Downloader` del host, de su origen y con el sha256 fijado en el código, el `planetiler.jar` oficial de la release de Planetiler (0.10.2, 93 MB, con todas sus dependencias) y el JRE de Temurin para su plataforma (Adoptium; Java 21, el que pide Planetiler 0.10.2; las versiones nuevas piden Java 25). El perfil de Protomaps, con nuestros parches, es un jar de 375 KB que solo depende de `planetiler-core`: lo compila la CI de este repositorio y va dentro de la propia librería del módulo (`module/build.rs`, con `GEKO_TILES_PROFILE`), que ya se publica firmada: así el módulo no tiene que buscar la carpeta de su paquete. El build lo escribe junto al JRE. En `build` se lanza como subproceso: `java -cp planetiler.jar:protomaps-basemap.jar com.protomaps.basemap.Basemap …`. Comprobado con La Rioja: las mismas teselas, byte a byte, que con el jar del perfil con dependencias; solo cambia la fecha de compilación de Planetiler en los metadatos. El lanzador de un solo fichero de Planetiler se descarta: es experimental y no fija nuestra versión del runtime.
- **Lo que necesita del sistema (2026-10-24): `libfontconfig1`.** El perfil lee las fuentes de pgf-encoding con `java.awt.Font`, y el JRE de Temurin, aun en modo *headless* (`-Djava.awt.headless=true`, que evita las librerías de X11), necesita el fontconfig del sistema: sin él, «Problem reading font data». En `debian:bookworm-slim`, la base de la imagen de geko, basta `libfontconfig1` (unos 1,5 MB con `libfreetype6`). **Pendiente en el host:** añadirlo a la imagen (`Dockerfile`, etapa `base`). Si falta, el build lo dice.
- **`tile_weights.tsv.gz` no hace falta:** Planetiler solo lo usa para una estadística del log (el tamaño medio de las teselas ponderado por el tráfico de OSM) y no lo baja si no se le pide (`download_osm_tile_weights=false`). No se descarga.
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
- Los estilos, fuentes y sprites se sirven como bytes estáticos del bundle, precomprimidos: el motor no sabe qué estilo es. De entrada, los de Protomaps con sus URLs apuntando a las propias («Estilos»).

### Estilos (2026-10-24)

- **Los de Protomaps**, del mismo commit que el perfil: 5 sabores (`light`, `dark`, `white`, `grayscale`, `black`) por 41 idiomas, 205 estilos. Sus capas las genera el código TypeScript de Protomaps (`@protomaps/basemaps`, BSD-3; el diseño es CC0): `ci/profile.sh` lo ejecuta en Docker (`planetiler/styles.ts`, con su `package-lock.json`) y deja `styles/<sabor>/<idioma>.json`, que `build.rs` mete en la librería comprimidas una a una (780 KB, 3,8 KB cada una). No hay Node en geko.
- **Compuestos al servirlos:** el módulo añade a las capas la fuente (`<url>/tilejson.json`, con `encoding: "mlt"` si las teselas son MLT), las fuentes y el sprite (`?v=<bundle>`), la primera vez que se piden en cada bundle, y los guarda comprimidos. `url` (`geko-tiles.toml`) es relativa por defecto; MapLibre exige absolutas la del sprite (`new URL`) y las de las teselas (las pide desde un *worker*, donde `/tiles/…` no se resuelve: «Failed to parse URL»), así que un cliente que cargue el estilo o `tilejson.json` por su URL necesita `url` absoluta. La demo del host lee `tilejson.json`, hace absolutas sus URLs con su origen y lo mete en la fuente del estilo.
- **Fuentes y sprites:** los de `protomaps/basemaps-assets` en un commit fijo (`[assets]` de `planetiler/profile.toml`), descargados pieza a pieza de `raw.githubusercontent.com` y comprobados con su sha256 (`planetiler/assets.tsv`, de `ci/assets.sh`): Noto Sans Regular, Medium e Italic y la Devanagari de pgf (256 rangos cada una; 252 de la Devanagari son enlaces simbólicos a Noto Sans Regular, que no se descargan), su licencia (OFL) y los sprites v4 de los 5 sabores (iconos de Tangram, MIT). Ni el zip de GitHub (sus bytes no están garantizados) ni un paquete propio. La etapa `assets` las empaqueta como las teselas (`tiles/assets/assets.bin`, mapeado, e `index.json`), con gzip si las hace menores (los glifos y el JSON; los PNG, tal cual) y cada contenido una vez.
- **Rutas:** `/tiles/styles.json` (sabores, idiomas y la plantilla de la URL), `/tiles/styles/{sabor}/{idioma}.json`, `/tiles/fonts/{fontstack}/{range}.pbf` (de una lista de fuentes, la primera que hay) y `/tiles/sprites/{nombre}`; fuentes y sprites, `immutable` con `?v=` del bundle en uso, como las teselas.
- **Nombre:** Protomaps pide otro nombre para teselas o estilos modificados, y nuestro perfil lleva parches: `tilejson.json` se llama «geko tiles» y nombra el esquema de Protomaps en su descripción; la atribución es la de sus estilos, «Protomaps © OpenStreetMap».
- **Estilos propios** (paso 9, desde el panel de control): un estilo es un JSON de MapLibre, datos. El panel puede generar las capas de un sabor propio con `@protomaps/basemaps` en el navegador, o editar un estilo entero, y geko lo serviría como los de Protomaps, sin regenerar teselas mientras pinte lo que llevan.

### Formato de las teselas: MVT, y MLT como opción (2026-10-24)

Las teselas son para cualquiera que use el motor, con el cliente que quiera (MapLibre, OpenLayers, QGIS, apps móviles, versiones antiguas…). MLT solo lo lee MapLibre GL JS desde la 5.12.0 (`encoding: "mlt"` en la fuente) y su especificación aún evoluciona, así que **MVT por defecto**. MLT, un 10 % menor con Baleares, queda como opción de `geko-tiles.toml` (`format = "mvt" | "mlt"`) para quien controle sus clientes: Planetiler genera los dos con `--tile_format` y el servicio solo copia bytes. TileJSON no tiene campo para el formato: `tilejson.json` lleva `"encoding": "mvt" | "mlt"` (fuera del estándar, el nombre de la opción de MapLibre), y el estilo pone `encoding: "mlt"` en la fuente.

### Varios extractos: fusionados antes, una sola ejecución (2026-10-24)

Medido con La Rioja y Navarra («Medidas»): una ejecución por extracto paga varias veces las entradas globales (un 77 % más de tiempo) y deja teselas compartidas con un 6 % de objetos repetidos, que una fusión de teselas no deja igual que una sola ejecución. Así que:

- **Una etapa en Rust fusiona los extractos del ámbito en un solo PBF:** los lee en orden (nodos, vías y relaciones, cada tipo por id, como los publica Geofabrik), se queda con una copia de cada objeto repetido y escribe el resultado. Determinista y con su sello: solo se repite si cambia algún extracto. Con un solo extracto no hace nada. Lee con [osmpbf](https://crates.io/crates/osmpbf) (MIT o Apache-2.0, el mismo lector que el geocoder) y escribe con un escritor propio (`module/src/pbf.rs`: protobuf a mano y zlib, bloques de 8.000 objetos de un tipo): determinista por construcción y sin otra dependencia. Solo escribe lo que Planetiler lee (posición, etiquetas, referencias y miembros; ni versiones ni fechas, que el perfil no usa). Se descartó [pbfhogg](https://docs.rs/crate/pbfhogg/0.5.0/source/README.md) (lee y escribe, pero es joven, 0.5, y solo se prueba en Linux).
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
5. ~~España~~: medida con el build (paso 4, «Medidas»): 14 min, 1,17 GB, 4 GB de *heap* para Planetiler.
6. **Las teselas de más de Planetiler** (paso 4, «Medidas»): de z8 en adelante, que los polígonos de osmdata solo entren en las teselas del ámbito, y no en toda la tesela z6 (un parche al perfil, o dos recortes: uno a las teselas z6 para z6–z7 y otro a la caja del ámbito para z8+). Ahorraría unos tres cuartos de las teselas que genera Planetiler.

## Pasos

1. **Prueba con medidas** (Baleares congelada del golden del geocoder; hecha. España, aplazada: «Por decidir», 5): el perfil de Protomaps con Planetiler en Docker. Tiempo y RAM del build; tamaño con MVT y MLT, con gzip, zopfli y brotli, y con zoom máximo 13 y 14; cuántas teselas se repiten; si dos ejecuciones dan los mismos bytes. Cierra los puntos de «Por decidir». Los resultados van a «Medidas», abajo.
2. **Puesta al día del repositorio** (hecho, 2026-10-24): la SDK en la etiqueta `sdk-1`, README y AGENTS.md como los del geocoder, el secreto `GEKO_PACK_SIGNING_KEY` (firma el módulo al publicarlo) y la decisión de licencias por escrito (abajo).
3. **Perfil** (hecho, 2026-10-24): Protomaps en el commit de `planetiler/profile.toml` con los cuatro parches de `planetiler/patches/` (Natural Earth por capas, la cadena vacía como valor ausente, osmdata ya recortado y ninguna descarga propia: qrank opcional, pgf-encoding dado). `ci/profile.sh` lo compila en Docker, con la fecha del commit, y da el mismo jar byte a byte; `ci/test.sh` (`profile`) comprueba que los parches se aplican y compila, y `ci/modules.sh` lo mete en el paquete del módulo. Con Baleares da las mismas teselas que la prueba con `--bounds=world`, en 45 s. `planetiler/profile.toml` fija también `planetiler.jar` y el JRE de Temurin (21.0.13+9), con sus sha256, para el paso 4.
4. **Build** (hecho, 2026-10-24): `plan`, `fetch` y `build` (`module/src/pipeline.rs`). Descarga con el `Downloader` del host (`geko-fetch`, como el geocoder) los extractos de Geofabrik (los mismos ficheros que el geocoder, en `raw/osm/`; la elección por país y región está en `module/countries.toml`), las 55 piezas de Natural Earth, osmdata, Daylight, pgf-encoding, qrank (opcional), `planetiler.jar` y el JRE, los fijados con su sha256 de `planetiler/profile.toml`. Las etapas, cada una con su sello en `<datos>/tiles/`: `scope` (las cajas del ámbito, del encabezado de cada extracto), `osmdata` (el recorte, `module/src/shapefile.rs`), `osm` (la fusión, solo con varios extractos), `runtime` (el JRE desempaquetado), `planetiler` (la clave lleva las versiones de Planetiler, del perfil y del JRE, el formato y lo que lee; no la memoria, los hilos ni el almacenamiento, que no cambian las teselas) y `bundle` (la conversión, `module/src/tileset.rs`, que es la parte del bundle, `tiles/`). Las licencias de cada fuente van al manifiesto, y `meta` lleva el formato, el tipo de tesela, la compresión y las cajas del ámbito para el servicio. Tras publicar, con `keep_downloads = false`, borra las descargas de datos y deja su ficha; una etapa que tenga que repetirse las vuelve a bajar. `module/examples/build.rs` lanza un build sin el host, para medir. Medido con Baleares, La Rioja y Navarra, y España («Medidas»).
5. **Servicio** (hecho, 2026-10-24): `module/src/serve.rs`. `load` lee el índice a RAM, mapea `tiles.bin` (`memmap2`), comprueba que cada entrada cae dentro y cambia de bundle de golpe (`arc-swap`); si el nuevo falla, sigue el de antes. `/tiles/{z}/{x}/{y}` (admite `.mvt`, `.pbf` y `.mlt` tras `y`): 200 con `Content-Encoding: gzip`, 204 si no hay tesela, 404 por encima del zoom máximo, 400 si las coordenadas no valen; `Cache-Control: public, max-age=31536000, immutable` solo si `?v=` es el bundle en uso, y `no-cache` si no (una URL sin versión o de otro bundle puede cambiar). `/tiles/tilejson.json` (TileJSON 3.0, `no-cache`): nombre, atribución y `vector_layers` del `metadata.json` de Planetiler, los zooms, `bounds` y `center` de las cajas del ámbito, `encoding` y las teselas en `<url>/{z}/{x}/{y}?v=<id del bundle>`, con `url = "/tiles"` de `geko-tiles.toml` (una URL absoluta para una CDN o un proxy con prefijo). Métricas: `geko_tiles_requests_total{result=tile|empty|error}`, `geko_tiles_sent_bytes_total`, `geko_tiles_loaded` y `geko_tiles_index_bytes`. `module/examples/serve.rs` lo mide sin el host («Medidas»).
6. **Estilos, fuentes y sprites** (hecho, 2026-10-24): `module/src/styles.rs` y `module/src/assets.rs` («Estilos»). Las descargas van en paralelo (`[data] parallel_downloads`) y las fijadas por sha256 cuya ficha ya lo tiene no se vuelven a pedir. La demo del host (rama `feature/tiles-demo` de geko) pinta las teselas propias con el estilo claro u oscuro, en el idioma del navegador, si `tiles` está activo, con MapLibre dentro de Leaflet; la imagen del host lleva `libfontconfig1` y `make start-dev` compila el módulo con su perfil y sus estilos.
7. **Comprobaciones** (hecho, 2026-10-25): el golden de Baleares, `ci/test.sh --variant golden` (`ci/golden.sh`, `ci/golden-snapshot.sh`, `module/examples/golden.rs`, AGENTS.md): construye sobre las descargas congeladas de `golden/snapshot` y compara con `golden/baseline.json` el número y el tamaño de las teselas por zoom, el sha256 de todas, el de las teselas de seis lugares en ocho zooms, el de `metadata.json` y el de las fuentes y sprites. Se congela solo lo que no fija `planetiler/profile.toml`, y osmdata recortado al ámbito: recortarlo otra vez da los mismos ficheros (`module/src/golden.rs`, con su prueba), así que 1,8 GB se quedan en 9 MB y el snapshot en 76 MB. Sin qrank (121 MB del mundo entero). Pruebas nuevas del servicio: estilos, fuentes (la lista de pilas, el enlace de la devanagari) y sprites con sus cabeceras, y peticiones raras a un bundle cargado (lo que la conformidad del SDK pregunta sin bundle). El índice (`tileset`, `serve`) y la fusión (`pbf`: objetos repetidos, orden, determinismo, un extracto desordenado) ya tenían las suyas. El golden es un job propio de la CI (`golden`), que lee el snapshot de `ghcr.io/candymaps/geko-tiles/golden` con el token del workflow.
8. **Publicación:** la primera versión publicada; `tiles` entra en `ci/modules` del host y vuelve a estar activo por defecto.
9. **Después, según las medidas:** brotli, MLT, el recorte al estilo y, con el panel de control, estilos propios del usuario.

## Licencias

- **El código** es AGPL-3.0-only, como el resto de geko. Planetiler corre en un proceso aparte y solo intercambia ficheros: es agregación (AGPLv3 §5), y ninguna licencia se contagia en ningún sentido.
- **Planetiler y el JRE no se redistribuyen:** cada instalación los baja de su origen (la release de Planetiler y Adoptium), como las fuentes de datos. Lo que lleva Planetiler (Apache-2.0, **LGPL** de GeoTools: `gt-shapefile`, `gt-epsg-hsql`, EDL de JTS) y el runtime (**GPLv2 con Classpath Exception**) llega con sus propias licencias, sin obligaciones de redistribución para nosotros. Solo se redistribuye el perfil de Protomaps (BSD-3) con nuestros parches, dentro del paquete del módulo, con su `LICENSE`. Pendiente de revisar con el punto 26 del host (un abogado antes de abrir el servicio).
- **Las teselas** son una obra producida (*Produced Work*) a partir de OSM (ODbL): llevan «© OpenStreetMap contributors» en `tilejson.json`, en `/v1/attribution` y en el estilo. Natural Earth es de dominio público; los polígonos de agua, datos de OSM.
- **Los estilos, las fuentes y los sprites:** el diseño de los estilos de Protomaps es CC0 (piden atribución, sin obligar), las fuentes Noto Sans son OFL 1.1 y los iconos de los sprites, de Tangram, MIT; cada uno va al manifiesto con su licencia. Las fuentes y los sprites se descargan de su origen; las capas de los estilos, generadas por código BSD-3, van en la librería del módulo.

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
5. **Índice propio:** con run-lengths, unos 180 KB para Baleares; sin ellos, 8,7 MB para España. Se queda sin run-lengths (paso 5, «Medidas»).

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

### El build del módulo: Baleares, La Rioja y Navarra, y España, 2026-10-24 (paso 4)

`module/examples/build.rs` en `debian:bookworm-slim` con `libfontconfig1` (como la imagen de geko), en WSL (16 hilos, 15 GB), con las opciones por defecto (`memory = "4g"`, `storage = "mmap"`, qrank) y los extractos de Geofabrik del día. El pico es el del proceso del módulo; el de Planetiler va en su propio proceso (su *heap*, de su log).

| Ámbito | Descarga | Recorte | Fusión | Planetiler | Conversión | Total | Teselas (distintas) | Bundle |
|---|---|---|---|---|---|---|---|---|
| Baleares (1 extracto, 47 MB) | — | 8 s | — | 64 s | 0,1 s | 71 s | 43.116 (6.919) | 31 MB |
| La Rioja y Navarra (2, 52 MB) | — | 8 s | 3,6 s | 71 s | 0,1 s | 85 s | 13.684 (9.259) | 56 MB |
| España (`spain` y `canary-islands`, 1,56 GB) | 61 s | 8 s | 96 s | 11 min 8 s | 3 s | 14 min | 565.277 (228.688) | 1,17 GB + 8,7 MB de índice |

- **Determinista:** dos builds de Baleares con `--force` dan los mismos bytes en todas las etapas, también tras cambiar de versión `zip` (el recorte) y la forma de guardar las cajas en el sello.
- **Incremental:** un build sin cambios reutiliza todas las etapas y tarda 0 s.
- **Memoria del módulo:** 28 MB el recorte (lee los zips en flujo), 62 MB la fusión de España (un bloque por extracto a la vez), 65 MB la conversión. Planetiler con España: *heap* de 3,6 GB de 4,2 GB en el pico, 13 núcleos de media; 4 GB bastan.
- **Fusión de España:** 215 millones de objetos en 96 s, solo 103 repetidos (Canarias apenas toca a España). La Rioja y Navarra: 100.357 repetidos de 7,2 millones, en 3,6 s. Un solo hilo: si hace falta, paralelizar la descompresión y la compresión de bloques.
- **Teselas de más en Planetiler:** el recorte de osmdata guarda las teselas z6 enteras que tocan el ámbito, porque el perfil usa los polígonos de agua y tierra desde z6 y esas teselas los necesitan completos. Pero Planetiler genera entonces todos los zooms de esos cuadrados: con España, 2,1 millones de teselas, de las que la conversión guarda 565.000 (con Baleares, 353.000 y 43.000). Es lo más caro que queda (unos 20 s de los 64 de Baleares). Ver «Por decidir».
- **Índice:** 16 B por tesela, sin run-lengths: 8,7 MB para España, en RAM. Con run-lengths, mucho menos (las repetidas son el 60 %); decidido con las medidas del servicio (paso 5): no hace falta.

### El servicio: España, 2026-10-24 (paso 5)

`module/examples/serve.rs` en `--release`, en WSL (16 hilos, 15 GB), con el bundle de España del paso 4 (565.277 teselas, 1,17 GB) y la caché de páginas ya caliente: llama a `handle` como el host, sin HTTP, con teselas al azar de las cajas del ámbito y todos los zooms de 0 a 14 igual de probables (23 KB de media por tesela: pesan las de zoom bajo).

| Hilos | Peticiones/s | p50 | p99 | p99,9 |
|---|---|---|---|---|
| 1 | 700.000 | 0,9 µs | 8,0 µs | 18,9 µs |
| 16 | 4,9 millones | 1,7 µs | 13,5 µs | 56 µs |

- **Carga:** 30 ms (leer el índice de 8,7 MB y comprobarlo entero); 11 MB de RSS antes de servir. Lo demás es `tiles.bin` mapeado, en la caché de páginas del sistema.
- **Contadores:** sin los dos contadores atómicos de cada tesela, 5,06 millones por segundo en vez de 4,86 (un 4 %, dentro del ruido): se quedan.
- **Run-lengths en el índice:** no compensan. El índice de España ocupa 8,7 MB y la búsqueda no es lo que cuesta: el tiempo se va en copiar el cuerpo. El servicio queda muy por debajo de lo que tarda el HTTP del host.
- Falta medirlo en frío (sin la caché de páginas) y por HTTP, con el host.

### Estilos, fuentes y sprites: Baleares, 2026-10-24 (paso 6)

- **Descarga** de las 793 piezas (1.024 rangos de fuentes menos 252 enlaces, su licencia y 20 sprites): 176 s una tras otra, **33 s** con 4 a la vez (`parallel_downloads` por defecto). Sin cambios, 0 s: las fijadas por sha256 con su ficha no se vuelven a pedir.
- **Empaquetado:** 1.045 piezas, 787 contenidos distintos, 11,5 MB → **6,3 MB** (gzip en glifos y JSON), en 285 ms; dos builds dan los mismos bytes.
- **Estilos:** 205, 780 KB en la librería. `light/es` sale con 4,3 KB en gzip; un rango de glifos, 45 KB; el sprite `light@2x.png`, 29 KB.
- **Servicio de Baleares** con las piezas cargadas: 2 ms en cargar, 7,3 millones de teselas por segundo con 16 hilos (p50 1,1 µs, p99 4,3 µs).

### El golden: Baleares, 2026-10-25 (paso 7)

- **Build del golden** sobre los datos congelados, todo rehecho: 45 s (el recorte de osmdata, 7 s; Planetiler, 37 s); con la compilación y el informe, 1 min 25 s. 0 descargas con la caché de lo fijado.
- **Determinismo:** con 3 hilos y `storage = "ram"` en vez de los de la máquina y `mmap`, el mismo informe byte a byte (Planetiler, 56 s). Con los zips de osmdata del día siguiente, también: los cambios no tocan Baleares.
- **Snapshot:** 76 MB (el extracto, 47 MB; Daylight, 36 MB; osmdata recortado, 9 MB).
- **Contenidos repetidos:** 43.116 teselas, 6.910 contenidos distintos por sus bytes, pero `tiles.bin` guarda 6.919: la conversión deduplica por el desplazamiento de Planetiler, que deja 9 contenidos dos veces (1,8 KB de 31 MB). No compensa cambiarlo.
