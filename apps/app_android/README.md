# app_android

Android `cdylib` independiente.

- Consume `core_logic` para modelos y selectores de formato.
- Usa formatos directos (`best[ext=mp4]` y `bestaudio`) para evitar FFmpeg externo en descargas Android.
- Declara FFI hacia `libmpv.so`, empaquetada desde `assets/android/<abi>`.
- El puente JNI de `src/chaquopy.rs` llama a `MainActivity.extractMedia`.

## Backend predeterminado: un solo Python con Chaquopy

Construir desde la raiz del repositorio (no desde `.git`):

```powershell
.\build-android-chaquopy.ps1
```

Requiere JDK 17 o 21, SDK Android 36, NDK y Python 3.13 en el equipo de build.
El Gradle Wrapper incluido fija Gradle 8.9 y verifica el SHA-256 de su
distribucion. No hace falta instalar Gradle globalmente.
El Python del equipo solo prepara dependencias: no agrega otro runtime al APK.
`CHAQUOPY_BUILD_PYTHON` permite indicar su `python.exe` si no es detectable.
El script tambien detecta las herramientas locales preparadas en
`%LOCALAPPDATA%\VisualRustMPV\build-tools` (JDK 17 y `Python313`).

Si PowerShell bloquea scripts locales, ejecutar con una politica limitada
al proceso, sin cambiar la configuracion global:

```powershell
powershell.exe -NoProfile -ExecutionPolicy RemoteSigned -File .\build-android-chaquopy.ps1
```

El script compila **solo** `app_android` con la feature `chaquopy` y ensambla
el modulo Gradle. El manifest carga `libapp_android.so`, el nombre generado
por Cargo. El script comprueba los codigos de salida antes de anunciar exito.
El APK debug queda en `android/app/build/outputs/apk/debug/app-debug.apk`.
Antes de anunciar exito, el script inspecciona el ZIP del APK: exige un unico
`lib/arm64-v8a/libpython3.13.so` y rechaza los assets/ejecutables del backend
de subproceso. Las exclusiones usan `androidResources.ignoreAssetsPatterns`
y `packagingOptions.jniLibs.excludes`; las exclusiones del source set no
eliminaban el script del APK en la validacion real.

Flujo: arranque -> hilo persistente `yt-dlp-worker` -> `warmUpYtDlp`;
Play -> cola `mpsc` -> JNI -> `MainActivity.extractStreamUrl` ->
`yt_dlp_runner.extract_stream_url` -> URL HTTP(S) -> canal por peticion -> libmpv.
El puente carga `MainActivity` con `Context.getClassLoader().loadClass(...)`
y conserva una referencia global a esa clase. `ndk-context` proporciona
`Application`, no la Activity; su clase no contiene `extractMedia`.
No usa `FindClass` desde el worker nativo, donde el class loader puede no
encontrar las clases de la app. Las excepciones Python/Java se convierten
en errores visibles y se registran, sin fallback silencioso a Piped/Invidious.

Chaquopy administra Python 3.13, la stdlib y `yt-dlp`, configurados en
`android/app/build.gradle`. Solo se mantiene el modulo
`android/app/src/main/python/yt_dlp_runner.py`; no hay que extraer scripts,
aplicar chmod ni proporcionar un segundo Python. Gradle excluye los assets
del backend de subproceso y sus ejecutables. El script de build rechaza
runtimes Python alternativos en los directorios nativos de entrada.

### Worker Python persistente

Al arrancar, `chaquopy::start_worker` crea **un solo** hilo por proceso
(`OnceLock`), adjunto a la JVM toda su vida. Primero llama a
`MainActivity.warmUpYtDlp` -> `yt_dlp_runner.warm_up`: importa yt-dlp,
verifica QuickJS-ng y crea un `YoutubeDL` reutilizable por modo con el
extractor de YouTube ya instanciado. El estado muestra
`yt-dlp precargado en N s` (~2 s en el telefono API 31). Cada Play encola
un trabajo; se procesan en serie y responden por un canal propio, de modo
que la GUI no se bloquea y Detener sigue descartando el resultado pendiente.
Los errores normales de extraccion conservan la instancia; los inesperados
la descartan y se recrea en el siguiente Play.

Medicion en el dispositivo: de ~33 s por extraccion, ~29 s eran QuickJS
preprocesando el player de YouTube (~2.5 MB) en cada llamada, porque
yt-dlp-ejs desactiva su cache de player preprocesado (archivos grandes,
sin rotacion). El runner la activa con `cachedir=<cacheDir>/yt-dlp`,
conserva solo los dos players mas recientes (~4.3 MB cada uno) y los borra
tras cualquier error de extraccion. Resultado: ~33 s la primera vez que
aparece un player nuevo y ~4.5-5.5 s despues, incluso tras reiniciar la app.

Tras la precarga, el worker llama a `warmUpYouTubePlayer` -> `warm_player`:
consulta en `iframe_api` el id del player actual (pocos KB) y solo si no esta
en cache extrae un video publico corto, de modo que esos ~33 s ocurren en
segundo plano al abrir la app y no al pulsar Play (medido: 0.4 s si esta al
dia, 34.6 s con player nuevo). Ademas, `src/prefetch.rs` resuelve la URL de
YouTube cuando lleva 700 ms sin cambiar, para el modo elegido. El resultado
vale 5 minutos y el estado muestra `Enlace listo · pulsa Reproducir`. Play lo
envia a `loadfile` al instante (medido: 1 ms). Si la precarga sigue en curso,
Play adopta esa misma peticion en vez de lanzar otra. Las precargas en cola
cuya URL o modo cambiaron se descartan antes de tocar la red. Un fallo de
precarga solo se registra en el log, y Play vuelve a intentarlo normalmente.

### Actualizar yt-dlp sin reinstalar

Con el worker ya listo, un hilo `yt-dlp-update` consulta como maximo una vez
cada 24 h a que version apunta `releases/latest` en GitHub. Si es mas nueva,
`yt_dlp_update.py` descarga de PyPI el wheel de yt-dlp y el de la version
exacta de `yt-dlp-ejs` que fija. Antes de usarlos comprueba:
- que vengan de `files.pythonhosted.org`;
- el tamano y el SHA-256;
- las rutas y el tamano descomprimido.

Solo extrae `yt_dlp/` y `yt_dlp_ejs/` en `files/yt-dlp-update/v/<version>`.

En el siguiente arranque, `activate()` pone esa version al frente de `sys.path`
antes de importar yt-dlp. Nunca cambia modulos en caliente. La version queda en
prueba hasta que una extraccion real tras el calentamiento la confirma. Se
rechaza y vuelve la incluida si:
- falla esa extraccion con red;
- no importa;
- pasan dos arranques sin poder verificarla.

Reinstalar el APK descarta lo descargado. La interfaz incluye el boton
`Actualizar yt-dlp` junto al selector de modo: fuerza una comprobacion
inmediata (ignora las 24 h) y se desactiva mientras busca.
Comprobado en el telefono con 2026.10.21 empaquetado:
1. Descargo 2026.10.26 en 2.3 s.
2. Al reiniciar la verifico con una extraccion de 6.8 s; el player se volvio a
   preparar en 41 s en segundo plano.
3. Al reinstalar se descarto.

### JavaScript para YouTube: QuickJS-ng

El APK incluye **QuickJS-ng 0.12.1** como ejecutable Android PIE arm64,
con nombre `libqjs.so` para que el instalador lo extraiga a
`ApplicationInfo.nativeLibraryDir`. No se copia ni ejecuta desde `files/`,
algo restringido por Android 10+; tampoco es otra instalacion de Python.
`useLegacyPackaging=true` mantiene la extraccion del ejecutable nativo.

El script de build descarga el codigo fuente oficial con version y SHA-256
fijados, compila con Clang del NDK para API 24 y alineacion ELF de 16 KiB,
elimina simbolos y empaqueta su licencia en
`assets/licenses/quickjs-ng.txt`. Requiere `ANDROID_NDK_HOME` y `tar` en
Windows. Reutiliza el ejecutable si no cambian la fuente, el NDK ni las
opciones. Un build Gradle directo verifica que el ejecutable y la licencia
hayan sido preparados; despues de `clean`, ejecutar el script completo.
El script inspecciona el APK y el archivo de dependencias Chaquopy para
exigir tanto `libqjs.so` y su licencia como los dos scripts locales EJS.

`MainActivity` pasa la ruta instalada al runner. Antes de extraer, Python
ejecuta una comprobacion JavaScript con timeout de 10 segundos (una vez
por ruta/proceso); ausencia, timeout, fallo de ejecucion o salida invalida
se convierten en errores visibles. Para dejar la GUI limpia, el texto de estado
de depuracion y la linea de salida de audio se quitaron de pantalla y quedan en
logs. yt-dlp recibe explicitamente
`js_runtimes={"quickjs": {"path": runtime_path}}`, sin buscar Deno o Node.

Gradle fija el par compatible **yt-dlp 2026.10.26 / yt-dlp-ejs 0.8.0**.
EJS contiene los scripts del solver: instalar solo el motor no basta.
`remote_components` permanece vacio; no se habilita descarga de componentes
JavaScript desde GitHub/npm durante la extraccion. Al actualizar yt-dlp,
actualizar EJS segun sus metadatos de dependencias y repetir las pruebas.
El timeout de comprobacion no es un limite total para los desafios EJS ni
para la extraccion; esta sigue ejecutandose en el worker sin bloquear la GUI.

El extractor conserva los selectores de formato existentes: Audio usa
`bestaudio[ext=m4a]/bestaudio[ext=webm]` y Video
`best[ext=mp4][height<=720]`. Resuelve sin descargar ni fusionar con FFmpeg.
Si el formato no existe, el error llega a la interfaz. Tanto Python como Rust
validan que haya una sola URL HTTP(S) antes de `loadfile`.

### Estado real de libmpv y audio

`src/playback.rs` consume eventos con `mpv_wait_event(..., 0)` sin bloquear
la GUI y con un maximo de 128 eventos por frame. Los datos FFI se copian o
consumen antes de pedir el siguiente evento, respetando su vida util.

- Aceptar `loadfile` muestra **Cargando stream**, no Reproduciendo.
- `FILE_LOADED` indica que el medio abrio; `PLAYBACK_RESTART` confirma que
  comenzo/reanudo la reproduccion.
- `pause` y `paused-for-cache` distinguen pausa y buffering.
- `END_FILE` muestra finalizacion o error con `mpv_error_string`.
- `time-pos`, `duration` y `current-ao` actualizan tiempo, duracion y salida
  de audio. El tiempo se registra aproximadamente cada cinco segundos.
- Logs de error/advertencia de mpv ayudan a diagnosticar red, TLS y audio.
  Los tokens que contienen URLs se sustituyen por `[URL]`. Los fallos de
  certificados, HTTP 403/404 y conexion se resumen en la interfaz.

La verificacion real encontro que libmpv rechazaba el certificado del stream.
Gradle ahora incluye `certifi` dentro del mismo Python de Chaquopy.
`MainActivity.caBundlePath()` entrega la ruta PEM extraida y Rust configura
`tls-ca-file` antes de inicializar mpv. Se mantiene **`tls-verify=yes`**:
no se desactiva TLS ni se ignoran certificados. Si el bundle falta, la
inicializacion falla explicitamente. Este bundle usa las CA publicas de
certifi; no incorpora automaticamente CA privadas instaladas por el usuario.

Se selecciona **OpenSL ES** (`ao=opensles`). En el dispositivo probado, el
backend AudioTrack de libmpv requeria registrar una JVM y fallaba; OpenSL ES
funciono sin agregar otro puente JNI. Si OpenSL ES no esta disponible, el
error se comunica: no se elige una salida nula. `ytdl=no` impide que mpv intente
ejecutar su propio extractor, y `network-timeout=15` limita esperas de red.

### Historial y reanudacion

`src/history.rs` guarda los ultimos 50 elementos en `files/history.json`:
titulo, canal, modo, posicion y duracion. En YouTube la clave es el ID del
video, asi que `watch?v=`, `youtu.be`, `shorts` y `live` comparten posicion.

La posicion se guarda:
- cada 5 s de avance;
- una vez al pausar;
- al detener o terminar;
- tambien desde `nativeTick` del servicio, asi que funciona con la pantalla
  bloqueada.

Al volver a reproducir, `loadfile` recibe `start=<s>` y el estado muestra
`Reanudando en m:ss`. No se reanuda:
- antes de 15 s;
- cerca del final (ultimos 15 s o 5 %), porque cuenta como visto;
- en directos sin duracion.

Bajo el reproductor aparece la lista: tocar un elemento lo reproduce en su
modo, y cada uno tiene boton para quitarlo. "Borrar todo" pide confirmacion.

Comprobado en el telefono con Big Buck Bunny:
1. Se detuvo en 44.6 s y se cerro la app por completo.
2. La lista mostro `continuar en 0:44 / 10:34`, y al tocarla reanudo en
   `time-pos=44.64s`.
3. Con la pantalla apagada el archivo siguio avanzando (96.9 s).

### Video integrado y controles

`src/video.rs` usa la API de render de libmpv en un contexto **OpenGL ES 3**
EGL separado, con un Pbuffer de 1 x 1 y texturas compartidas con eframe/Glow.
mpv dibuja en un framebuffer RGBA8 propio y el callback egui compone su textura
con un shader, respetando el rectangulo de video y su clipping. Se sincronizan
las lecturas y escrituras GPU con `glFinish` antes de cambiar de contexto.
Esto evita que el estado GL de mpv interfiera con egui, como ocurria con
video negro y errores GL en el Redmi Note 8 Pro (Mali-G76).
No se crea otra ventana
ni una SurfaceView superpuesta: los controles y las barras del sistema
mantienen sus coordenadas. mpv conserva la proporcion de imagen y agrega
bandas negras cuando el medio no coincide con el area 16:9.

La instancia usa `vo=libmpv`. Cada `loadfile` lleva una opcion local:
`vid=no` en Audio y `vid=auto` en Video. Los selectores se bloquean durante
la resolucion para no mezclar el formato solicitado y el modo cargado.
Seleccionar otro modo no cambia el medio actual hasta pulsar Play.
El video usa decodificacion por software (`hwdec=no`,
`gpu-hwdec-interop=no`); MediaCodec/aceleracion por hardware quedan pendientes.
Se desactiva el OSD de mpv, sustituido por los controles egui.

Los controles vectoriales Play, Pausa y Stop usan iconos estandar y botones
tactiles de 40 x 40 puntos. Play reanuda el medio actual; Stop lo detiene
y un nuevo Play comienza desde cero. La URL directa se conserva solo en
memoria durante cinco minutos para no repetir la extraccion al reiniciar.
Un error de reproduccion obliga a resolver de nuevo. Stop durante una
extraccion descarta su resultado, sin matar Python ni iniciar otro worker
hasta que termine. La URL y los selectores quedan bloqueados mientras tanto.
El reloj muestra minutos/segundos y horas cuando corresponda.
En vertical, los botones de reproduccion y pantalla completa, el reloj y
el deslizador quedan anclados al borde inferior del area util, por encima
de la barra de navegacion Android. La URL y el selector Audio/Video siguen
arriba, y el video utiliza el espacio entre ambos sin solapar los controles.
En ambas orientaciones forman una sola fila centrada verticalmente:
Play, Pausa, Stop, pantalla completa, deslizador expansible y reloj.
El deslizador reserva el ancho real del reloj, incluso al mostrar horas.
Tras mas de 3 segundos sin actividad durante la reproduccion, la fila
se oculta; mover el puntero, tocar o usar el teclado la vuelve a mostrar.
Un puntero inmovil no reinicia el plazo. Mientras se mantiene pulsado o
se arrastra el deslizador no se oculta. Pausa, Stop, fin o error mantienen
los controles visibles.
Debajo del mensaje de estado aparece una barra indeterminada animada durante
la resolucion del enlace, la preparacion del stream y el buffering. No muestra
porcentajes porque esas operaciones no informan un progreso total medible.
Se oculta al reproducir, pausar, terminar o fallar; si Stop descarta una
extraccion en curso, sigue indicando trabajo hasta que el worker finaliza.

Play, Pausa, Stop y el deslizador de posicion usan comandos asincronos,
al igual que `loadfile`, para no esperar al core de mpv en el hilo de render.
`COMMAND_REPLY` comunica los errores a la GUI. El deslizador envia el seek
al soltarlo o editar el valor; no genera una busqueda por cada frame del
arrastre. La posicion presentada sigue viniendo de `time-pos`.
Play inicia el nuevo medio sin heredar una pausa manual del anterior.
Al reiniciar el estado de carga se conservan las propiedades persistentes
observadas (pausa y salida de audio): mpv no vuelve a notificarlas si no
cambiaron. Una prueba de regresion cubre este caso.

El boton de pantalla completa oculta las barras del sistema y solicita
orientacion horizontal por sensor (ambos lados). Al ocultarse los controles,
el video ocupa toda el area util sin reservar espacio para la fila inferior.
El boton de restaurar vuelve a vertical y
restablece las barras, sin recrear Activity ni mpv: conserva posicion y
pausa. `src/screen.rs` solicita el cambio mediante JNI en el hilo UI Java,
y el manifiesto declara los cambios de orientacion y tamano.
Atrás tambien sale de pantalla completa conservando posicion y pausa.
NativeActivity entrega la tecla como `BrowserBack`; el parche local de
`egui-winit` 0.29.1 en `vendor/egui-winit` la traduce a Escape solo en
Android, y la GUI la consume solo en pantalla completa. El parche se
selecciona desde el Cargo.toml raiz, sin modificar la cache global ni
cambiar la traduccion de teclas Windows. Se incluye un callback de Atrás
predictivo para API 33+, todavia no validado en dispositivo.

Durante video cargado y en reproduccion, `FLAG_KEEP_SCREEN_ON` evita que se
apague la pantalla (tambien durante buffering). No se activa para Audio ni
durante la extraccion inicial; se libera al pausar, detener, terminar o fallar.
El flag se actualiza mediante JNI en el hilo UI, solo al cambiar el estado.
`onPause` lo limpia inmediatamente y `onResume` lo restaura solo si corresponde.
No se cambia el timeout global de Android. El wake lock parcial del servicio
mantiene la CPU, no la pantalla, durante la reproduccion en segundo plano.
En el telefono API 31 se comprobaron los flags reales de ventana: activo
durante Video (vertical y horizontal), liberado en pausa, Stop, EOF, Audio
y segundo plano, restaurado al volver reproduciendo y desactivado al
volver tras una pausa manual. Atrás se verifico tanto reproduciendo como
en pausa; en este ultimo caso se conservo el mismo fotograma y tiempo,
sin cambiar el proceso ni volver a resolver el enlace.

El render no espera al instante de presentacion en el hilo GUI. Se solicita
un frame aproximadamente cada 16 ms en modo Video (50 ms en Audio).
El framebuffer se recrea solo al cambiar su tamano. Los fallos de render
se registran y muestran, y detienen el medio en vez de continuar con audio
silenciosamente. Si no se puede inicializar OpenGL ES 3, el modo Audio sigue
disponible y Video informa el error.

`MainActivity.onPause/onResume` notifican a `src/lifecycle.rs` por JNI:
la clase registra la biblioteca con `System.loadLibrary("app_android")`,
porque el `dlopen` de NativeActivity por si solo no permite resolver estos
metodos nativos Java.
Al bloquear el telefono o ir a segundo plano, el audio continua tanto en
Audio como en Video. Solo se desactiva la pista de video (`vid=no`) mientras
no hay superficie visible; al volver se restaura con `vid=auto`, sin
modificar pausa ni posicion. Una pausa manual se conserva.
El estado y la cola de eventos mpv se comparten bajo mutex entre GUI y
servicio: el servicio sigue consumiendo eventos incluso con egui suspendido,
sin perder EOF, errores ni cambios de posicion. El handle se desregistra
antes de destruir mpv.
eframe conserva el contexto GL al recrear la superficie Android; el render
y sus texturas se liberan en `on_exit`, antes de destruir el core.

`PlaybackService` es un servicio en primer plano de tipo `mediaPlayback`,
iniciado por la carga de un medio desde la interfaz visible. Usa la misma
instancia libmpv, no un reproductor adicional. Una `MediaSession` publica
posicion, duracion y estado, con Pausa/Reanudar/Detener en la notificacion
y en los controles multimedia del bloqueo. Cada carga envia titulo, canal y
hasta tres miniaturas HTTPS (`MediaInfo` validado en `src/stream_url.rs`);
el servicio descarga en segundo plano la primera imagen valida (timeout 8 s,
maximo 4 MB, reducida a 512 px) y la publica como artwork de la
`MediaSession` e icono grande de la notificacion. Una carga nueva invalida
la descarga anterior. Las URL directas muestran nombre de archivo y host sin
miniatura. El servicio solicita foco de
audio; las interrupciones transitorias pausan y pueden reanudar al recuperar
el foco, mientras que perdida permanente o desconexion de audifonos pausan.
La pausa explicita desde la notificacion cancela la reanudacion automatica.
Un `PARTIAL_WAKE_LOCK` se libera en pausa, Stop, EOF, error o destruccion
del servicio. Se declaran `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_MEDIA_PLAYBACK`
y `WAKE_LOCK`; no se altera la optimizacion global de bateria.
Stop, EOF y errores retiran el servicio y la notificacion. Quitar la tarea
de recientes detiene la reproduccion; no se reinicia tras force-stop o
muerte del proceso (`START_NOT_STICKY`). Bloquear o pulsar Inicio no equivale
a quitar la tarea. Las restricciones adicionales de bateria del fabricante
y versiones Android distintas del dispositivo probado requieren validacion.

Se verificaron imagen real de YouTube, bandas laterales correctas, pausa,
busqueda con cambio visible del fotograma y retorno desde segundo plano en
el dispositivo API 31. La escucha humana del modo Audio fue confirmada por
el usuario, quien tambien confirmo la sincronizacion audiovisual (lipsync).
La entrada horizontal inmersiva y la restauracion vertical se comprobaron
en API 31 conservando el fotograma pausado. Tambien se verificaron retorno
desde segundo plano en pantalla completa, Stop -> Play sin repetir yt-dlp
y descarte de una extraccion pendiente tras pulsar Stop. Otros dispositivos, el giro
fisico entre ambos lados horizontales y Atrás predictivo siguen pendientes.
La biblioteca empaquetada tambien emite avisos de Fontconfig al abrir video;
no impiden la imagen. Fuentes/subtitulos nativos aun no estan configurados.
En el APK final se repitio Video en pausa -> Audio: el reloj avanzo por
encima de cinco segundos, la salida siguio siendo OpenSL ES y el area de
video desaparecio. La suite Rust tiene 22 pruebas aprobadas, incluidas las
de margenes, ciclo de vida, reloj, Stop, estado para pantalla encendida y
reemplazo de un medio pausado.

### Area util de la interfaz

El panel egui respeta `AndroidApp::content_rect()` y el tamano de la
ventana nativa. Convierte los margenes fisicos a puntos mediante
`pixels_per_point()` y los suma al espaciado normal del panel. Asi el
titulo y los controles quedan fuera de las barras del sistema, sin
ocultarlas ni depender de una altura fija. Se consulta en cada frame,
porque winit 0.30 no propaga los cambios de ese rectangulo a egui.
Si Android ya ajusto la superficie, no se agrega margen duplicado.
La comprobacion visual en el dispositivo API 31 confirmo que la barra de
notificaciones ya no se superpone al titulo.

Las llamadas de red tienen `socket_timeout=10`, `retries=1` y
`extractor_retries=1`. **No es un timeout total de extraccion**: Python se
ejecuta dentro del proceso, por lo que no se intenta matarlo ni se lanzan
resoluciones adicionales mientras siga ocupado. La GUI sigue repintandose.
Cerrar la interfaz no cancela forzosamente el worker.

`cargo-apk` por si solo no empaqueta Chaquopy ni `MainActivity`: para este
backend usar Gradle. Los backends anteriores quedan disponibles como
alternativas explicitas; no se enlazan ni inicializan junto a Chaquopy.
El resolver HTTP anterior puede comprobarse con `--no-default-features`.
La extraccion real de YouTube sigue dependiendo de los requisitos de la
version instalada de yt-dlp, incluidos posibles desafios JavaScript.

Validacion local (las pruebas Python usan un extractor simulado, sin red):

```powershell
cargo test -p app_android --lib
cargo ndk -t arm64-v8a --platform 24 check -p app_android
python -B -m unittest discover -s android/tests -v
```

### Comprobacion en dispositivo

Se genero e instalo el APK arm64 con Chaquopy 17.0.0, Python 3.13 y
yt-dlp 2026.10.26, y se comprobo el arranque en Android API 31. La prueba
desde la GUI resolvio un enlace YouTube en modo Audio y produjo los logs
`Chaquopy resolvio una URL HTTP(S)` y
`URL directa enviada a libmpv mediante loadfile`. Esto confirma el puente
y la aceptacion del comando, no certifica por si solo audio audible ni
video renderizado.

En la comprobacion posterior se corrigio TLS y se observaron
`FILE_LOADED`, `PLAYBACK_RESTART`, `time-pos=0.00s`, `time-pos=5.00s` y
`current-ao=opensles`. Android AudioFlinger mostro una pista activa de la
app a 44100 Hz. Esto verifica avance y entrega al sistema de audio, pero
no confirma subjetivamente el sonido por el altavoz (volumen/ruta/mute).

En el APK final con `ao=opensles` se repitio la prueba negativa con
`https://127.0.0.1:1/test.mp3`: la GUI mostro el error de conexion y
**No se pudo conectar al servidor**, sin anunciar reproduccion.
Sin reiniciar la app, se cargo `https://youtu.be/jNQXAC9IVRw` y se verifico
la recuperacion: estado **Reproduciendo**, duracion de 19.1 segundos,
salida `opensles` y avance registrado de 0 a 5, 10 y 15 segundos.
AudioFlinger confirmo la pista activa de la app a 44100 Hz y al terminar
la GUI mostro **Reproduccion finalizada**. No reaparecio el error de JVM
del backend AudioTrack.

La advertencia original de runtime JavaScript ausente motivo la inclusion
de QuickJS-ng y EJS. Esto no elimina otros requisitos posibles de YouTube
(autenticacion, restricciones regionales o tokens del proveedor).
En el dispositivo API 31 se verifico ejecucion JavaScript real desde
`nativeLibraryDir` y un proceso hijo `libqjs.so` durante la resolucion.
Dos extracciones consecutivas del enlace de prueba terminaron correctamente,
sin la advertencia de runtime ausente ni errores EJS, y continuaron hasta
`FILE_LOADED`, `PLAYBACK_RESTART` y avance del reloj. Cada resolucion tardo
aproximadamente 34 segundos en ese telefono: QuickJS-ng reduce el peso del
motor, pero no garantiza la velocidad de motores con JIT. La GUI siguio
respondiendo mientras el worker resolvia. La escucha fisica queda pendiente.

Si `adb install -r` informa `INSTALL_FAILED_UPDATE_INCOMPATIBLE`, la app
existente esta firmada con otra clave. Reutilizar su clave de desarrollo
o solicitar autorizacion antes de desinstalar: la desinstalacion elimina
los datos locales. Los siguientes APK de este build usan la misma clave
debug de Gradle y pueden instalarse con `adb install -r`.

## yt-dlp mediante subproceso

Backend opcional, independiente de Chaquopy y de la C-API `native-python`:

```powershell
cargo apk build --manifest-path apps/app_android/Cargo.toml --lib --no-default-features --features yt-dlp-subprocess
```

No combinar esta feature con `chaquopy` ni `native-python`. La opcion Audio utiliza `-f bestaudio`; Video utiliza `-f best`,
un unico formato con audio y video, no `bestvideo+bestaudio`. `best` puede no
estar disponible para algunos videos: ese error se comunica sin fallback oculto.

### Restriccion importante de Android

La app tiene target SDK 36. Desde Android 10, para apps con target >=29,
no se puede ejecutar mediante `execve` un archivo de la carpeta privada
escribible, aunque tenga permisos 0700. Por ello **no** se ejecuta
`/data/data/com.visualrustmpv.app/files/yt-dlp` directamente. No reducir el
target SDK ni asumir que ejecutar Python embebido demuestra que `execve` funciona.
Referencia: [restricciones de ejecucion de Android 10](https://developer.android.com/about/versions/10/behavior-changes-10#execute-permission).

La solucion soportada separa datos e instrucciones nativas:

- Script y modulos Python: assets -> `Context.getFilesDir()` (normalmente
  `/data/user/0/com.visualrustmpv.app/files`, alias de `/data/data/.../files`).
- Ejecutable ELF Android PIE compilado con NDK/Bionic: `jniLibs` ->
  `ApplicationInfo.nativeLibraryDir`, instalado por PackageManager.
- `extractNativeLibs=true` y `useLegacyPackaging=true` garantizan un archivo
  ejecutable real, no una entrada comprimida/direccion dentro del APK.

### Opcion A: Python ejecutable + script

```text
android/app/src/main/
  assets/
    yt-dlp
    python-runtime/
      manifest.txt
      lib/python313.zip
      site-packages/
        yt_dlp/...
        certifi/...                 # incluir cacert.pem
        ...                         # dependencias de la version fijada de yt-dlp
  jniLibs/arm64-v8a/
    libpython_exec.so               # ejecutable PIE, NO libpython3.13.so
    ...                            # bibliotecas NDK necesarias
```

Tambien se admiten bibliotecas en `assets/android/arm64-v8a`, ya declarado
como directorio de runtime nativo. Estos assets son para `cargo-apk`;
el modulo Gradle actual los excluye porque esta dedicado a Chaquopy.
No colocar `libpython_exec.so` simultaneamente en ambos directorios.

`manifest.txt` enumera **todos** los archivos de datos relativos a
`python-runtime`, uno por linea; no se genera automaticamente. Ejemplo
abreviado (completar con todos los modulos, datos y dependencias):

```text
lib/python313.zip
site-packages/yt_dlp/__init__.py
site-packages/yt_dlp/__main__.py
site-packages/certifi/__init__.py
site-packages/certifi/core.py
site-packages/certifi/cacert.pem
```

`python313.zip` debe contener los modulos de la stdlib directamente en la raiz,
incluido `encodings`. El ejecutable debe incorporar los modulos nativos que
necesite Python/yt-dlp como built-ins (por ejemplo `_ssl`, `_socket`, `zlib`,
`_hashlib`, `_struct`, `math`, etc.), o tener una estrategia de carga de
extensiones desde el directorio nativo instalado. Este instalador rechaza
`.so` en el manifiesto: no resuelve empaquetado/importacion de extensiones.
No basta renombrar `libpython3.13.so`: se necesita un ejecutable con entrada
`main`. Usar el mismo ABI/API que la app, dependencias Bionic y RUNPATH
`$ORIGIN` cuando corresponda. Se configura ademas `LD_LIBRARY_PATH`.
No sirven binarios Windows, Linux glibc/musl, ni ejecutables Termux que dependan
de su prefijo privado. No se incluyen ni descargan artefactos nativos con
este cambio; deben proporcionarse y verificarse antes de crear el APK.

En la primera resolucion, `yt_dlp_bundle::prepare` extrae el runtime y el script
en un worker, y aplica `std::fs::set_permissions(..., 0o700)` a `files/yt-dlp`.
Las siguientes resoluciones conservan archivos identicos; una actualizacion
del APK reemplaza los distintos mediante archivo temporal + rename. No se
marca como completa una extraccion parcial; puede reintentarse. El script
se pasa como argumento de Python, por lo que su permiso de ejecucion **no**
es una forma de eludir las restricciones de Android.

La invocacion efectiva de Video equivale a:

```text
<nativeLibraryDir>/libpython_exec.so <filesDir>/yt-dlp \
  --ignore-config --no-playlist --no-progress \
  --socket-timeout 10 --retries 1 --extractor-retries 1 \
  -g -f best -- "https://www.youtube.com/watch?v=VIDEO_ID"
```

Se establecen `PYTHONHOME=<filesDir>/python`,
`PYTHONPATH=<filesDir>/python/site-packages`, `PYTHONNOUSERSITE=1` y
`PYTHONDONTWRITEBYTECODE=1`. No se usa un shell ni interpolacion de comandos.
Chaquopy por si solo no proporciona este ejecutable independiente.

### Opcion B: standalone Android

Empaquetar **solo** `libyt_dlp_exec.so` en lugar de `libpython_exec.so`.
El resolver selecciona explicitamente el unico backend empaquetado y registra
la eleccion. Si faltan ambos o existen ambos, comunica un error.
El standalone debe ser un ELF PIE realmente compatible con Android/Bionic;
los standalone Linux publicados por yt-dlp no deben asumirse compatibles.
No necesita los assets Python y recibe directamente los mismos argumentos
`-g -f best ...`. No se copia a `files/` para ejecutarlo.

### Timeout, salida y libmpv

`yt_dlp_process` captura stdout y stderr concurrentemente para evitar deadlocks,
conserva hasta 256 KiB por canal y drena el resto. Impone un timeout global de
45 segundos al proceso, independiente del timeout de sockets. En Android usa un
grupo de procesos separado y lo termina, incluidos posibles hijos que hereden
las tuberias; siempre espera/recolecta el hijo. No descarga medios ni inicia
FFmpeg. Si la version de yt-dlp requiere un runtime JavaScript para YouTube,
ese runtime tambien debe empaquetarse/configurarse como ejecutable Android
permitido; no se suministra automaticamente ni se garantiza la extraccion sin el.

Se exige codigo de salida cero, UTF-8 y exactamente una URL HTTP(S) con host,
sin credenciales ni caracteres de control. Los errores y stderr se registran;
no se registran las URLs firmadas resueltas en el camino del subproceso.
El canal `mpsc` devuelve el resultado a la GUI, que solicita repintados y evita
resoluciones simultaneas. Solo entonces `load_stream` llama a libmpv con:

```text
mpv_command(context, ["loadfile", stream_url, "replace", NULL])
```

El timeout de 45 segundos cubre el subproceso, no la instalacion de assets ni
la carga de libmpv. Al cerrar la interfaz, el worker ya iniciado puede continuar
hasta terminar o agotar su timeout.

### Validacion

```powershell
cargo test -p app_android --lib
cargo ndk -t arm64-v8a --platform 24 check -p app_android --no-default-features --features yt-dlp-subprocess
```

Las pruebas locales cubren instalacion/reinstalacion de assets, rechazo de
rutas fuera del bundle, captura limitada, stdout, stderr, salida no cero,
timeout y validacion de URL. En dispositivo falta verificar el ELF proporcionado,
TLS/certificados, la resolucion real y reproduccion con libmpv. Comprobar logs
con `adb logcat`, probar sin red, un ID invalido y cierre durante la resolucion.
El build Gradle actual esta dedicado a Chaquopy. No utilizar el script
Chaquopy para empaquetar el backend de subproceso.
