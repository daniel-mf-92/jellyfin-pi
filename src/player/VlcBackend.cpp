#include "VlcBackend.h"

#include <QCoreApplication>
#include <QDir>
#include <QFile>
#include <QProcessEnvironment>
#include <QThread>

#include "utils/Utils.h"

static const char* VLC_BIN = "/usr/bin/cvlc";
static const char* SOCKET_PATH = "/tmp/jmp-vlc.sock";
static const int POLL_INTERVAL_MS = 200;
static const int AUDIO_DESYNC_MS = -300;  // Pi5 HDMI audio pipeline compensation

///////////////////////////////////////////////////////////////////////////////////////////////////
VlcBackend::VlcBackend(QObject* parent)
  : PlayerBackend(parent)
{
  m_socketPath = QString::fromLatin1(SOCKET_PATH);

  m_pollTimer = new QTimer(this);
  m_pollTimer->setInterval(POLL_INTERVAL_MS);
  connect(m_pollTimer, &QTimer::timeout, this, &VlcBackend::pollStatus);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
VlcBackend::~VlcBackend()
{
  cleanup();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
bool VlcBackend::initialize()
{
  // Remove stale socket
  QFile::remove(m_socketPath);
  qInfo() << "VlcBackend initialized";
  return true;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::cleanup()
{
  killVlc();
  QFile::remove(m_socketPath);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::play(const QString& url, const QVariantMap& options)
{
  // Stop any existing playback
  if (m_process && m_process->state() != QProcess::NotRunning)
    killVlc();

  double startSecs = options.value("startTime").toDouble();
  launchVlc(url, startSecs);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::stop()
{
  if (!m_process || m_process->state() == QProcess::NotRunning)
    return;

  sendCommand("quit");
  m_pollTimer->stop();

  if (!m_process->waitForFinished(2000))
  {
    qWarning() << "VLC did not exit gracefully, killing";
    m_process->kill();
    m_process->waitForFinished(1000);
  }

  m_playing = false;
  m_paused = false;
  emit backendCanceled();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::pause()
{
  if (m_playing && !m_paused)
  {
    sendCommand("pause");
    m_paused = true;
    emit backendPaused();
  }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::unpause()
{
  if (m_paused)
  {
    sendCommand("pause");  // VLC RC toggles pause
    m_paused = false;
    emit backendPlaying();
  }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::seekTo(qint64 ms)
{
  double secs = ms / 1000.0;
  sendCommand(QString("seek %1").arg(secs, 0, 'f', 1));
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::setVolume(int vol)
{
  m_volume = qBound(0, vol, 100);
  // VLC volume: 0-512, where 256 = 100%
  int vlcVol = static_cast<int>(m_volume * 2.56);
  sendCommand(QString("volume %1").arg(vlcVol));
}

///////////////////////////////////////////////////////////////////////////////////////////////////
int VlcBackend::volume()
{
  return m_volume;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::setMuted(bool m)
{
  m_muted = m;
  if (m)
    sendCommand("volume 0");
  else
    setVolume(m_volume);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
bool VlcBackend::muted()
{
  return m_muted;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::setAudioTrack(int trackId)
{
  sendCommand(QString("atrack %1").arg(trackId));
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::setAudioDelay(qint64 ms)
{
  // VLC RC doesn't support runtime audio-delay changes easily;
  // the initial --audio-desync is set at launch.
  Q_UNUSED(ms);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::setAudioDevice(const QString& deviceName)
{
  // VLC RC doesn't support runtime audio device switching.
  // Audio device is set at launch via --aout and --alsa-audio-device.
  Q_UNUSED(deviceName);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
QVariant VlcBackend::getAudioDeviceList()
{
  // Not easily queryable via RC socket. Return empty list.
  return QVariantList();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::setVideoRectangle(int x, int y, int w, int h)
{
  // VLC manages its own fullscreen window — no rectangle needed.
  Q_UNUSED(x); Q_UNUSED(y); Q_UNUSED(w); Q_UNUSED(h);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::setSubtitleTrack(int trackId)
{
  sendCommand(QString("strack %1").arg(trackId));
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::setSubtitleDelay(qint64 ms)
{
  Q_UNUSED(ms);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::addSubtitleFile(const QString& path)
{
  Q_UNUSED(path);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::setRate(double rate)
{
  sendCommand(QString("rate %1").arg(rate, 0, 'f', 2));
}

///////////////////////////////////////////////////////////////////////////////////////////////////
qint64 VlcBackend::getPosition()
{
  return m_position;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
qint64 VlcBackend::getDuration()
{
  return m_duration;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
bool VlcBackend::isPlaying() const
{
  return m_playing && !m_paused;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::updateAudioConfiguration()
{
  // Audio config is baked into launch args. No runtime changes via RC.
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::updateVideoConfiguration()
{
  // Video config is baked into launch args.
}

///////////////////////////////////////////////////////////////////////////////////////////////////
QString VlcBackend::videoInformation() const
{
  QString info;
  QTextStream ts(&info);
  ts << "Backend: VLC (cvlc subprocess + RC socket)\n";
  ts << "Position: " << m_position << " ms\n";
  ts << "Duration: " << m_duration << " ms\n";
  ts << "Playing: " << (m_playing ? "yes" : "no") << "\n";
  ts << "Paused: " << (m_paused ? "yes" : "no") << "\n";
  ts << "Volume: " << m_volume << "\n";
  return info;
}

// ---------------------------------------------------------------------------
// Private implementation
// ---------------------------------------------------------------------------

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::launchVlc(const QString& url, double startTimeSecs)
{
  QFile::remove(m_socketPath);

  m_process = new QProcess(this);
  connect(m_process, QOverload<int, QProcess::ExitStatus>::of(&QProcess::finished),
          this, &VlcBackend::onProcessFinished);
  connect(m_process, &QProcess::started, this, &VlcBackend::onProcessStarted);

  // Wayland environment
  QProcessEnvironment env = QProcessEnvironment::systemEnvironment();
  env.insert("WAYLAND_DISPLAY", "wayland-0");
  env.insert("XDG_RUNTIME_DIR", "/run/user/1000");
  env.insert("QT_QPA_PLATFORM", "wayland");
  env.insert("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus");
  m_process->setProcessEnvironment(env);

  int netCacheMs = adaptiveNetworkCachingMs();
  int prefetchBytes = adaptivePrefetchBytes();

  QStringList args;
  args << "--fullscreen"
       << "--play-and-exit"
       << "--intf" << "rc"
       << "--rc-unix" << m_socketPath
       << "--rc-fake-tty"
       << QString("--audio-desync=%1").arg(AUDIO_DESYNC_MS)
       << QString("--network-caching=%1").arg(netCacheMs)
       << QString("--file-caching=%1").arg(300)  // local files: 300ms
       << QString("--live-caching=%1").arg(netCacheMs)
       << "--http-reconnect"
       << "--http-continuous"
       << "--no-video-title-show"
       << "--quiet"
       << "--input-fast-seek"
       << QString("--prefetch-buffer-size=%1").arg(prefetchBytes)
       << "--avcodec-threads=4"
       << "--avcodec-hw=any";

  if (startTimeSecs > 0)
    args << QString("--start-time=%1").arg(startTimeSecs, 0, 'f', 1);

  args << url;

  qInfo() << "VlcBackend: launching cvlc"
           << "net_cache=" << netCacheMs << "ms"
           << "prefetch=" << (prefetchBytes / (1024 * 1024)) << "MB"
           << "start=" << startTimeSecs << "s";

  m_position = static_cast<qint64>(startTimeSecs * 1000);
  m_duration = 0;
  m_playing = false;
  m_paused = false;
  m_waitingForWindow = true;

  m_process->start(QString::fromLatin1(VLC_BIN), args);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::onProcessStarted()
{
  qInfo() << "VlcBackend: cvlc process started (pid" << m_process->processId() << ")";

  // Wait for RC socket to appear, then connect
  QTimer::singleShot(300, this, [this]() {
    if (connectSocket(5000))
    {
      m_playing = true;
      m_pollTimer->start();
      emit backendPlaying();
      emit backendVideoPlaybackActive(true);

      // Focus VLC window on Wayland
      focusVlcWindow();
    }
    else
    {
      qWarning() << "VlcBackend: failed to connect to RC socket";
      emit backendError("Failed to connect to VLC control socket");
    }
  });
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::onProcessFinished(int exitCode, QProcess::ExitStatus status)
{
  Q_UNUSED(status);
  qInfo() << "VlcBackend: cvlc exited with code" << exitCode;

  m_pollTimer->stop();
  disconnectSocket();
  m_playing = false;
  m_paused = false;

  emit backendVideoPlaybackActive(false);

  if (exitCode == 0)
    emit backendFinished();
  else
    emit backendError(QString("VLC exited with code %1").arg(exitCode));

  m_process->deleteLater();
  m_process = nullptr;
  QFile::remove(m_socketPath);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::killVlc()
{
  if (!m_process || m_process->state() == QProcess::NotRunning)
    return;

  m_pollTimer->stop();
  sendCommand("quit");

  if (!m_process->waitForFinished(2000))
  {
    m_process->kill();
    m_process->waitForFinished(1000);
  }

  disconnectSocket();
  QFile::remove(m_socketPath);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
bool VlcBackend::connectSocket(int timeoutMs)
{
  if (m_socket)
  {
    m_socket->deleteLater();
    m_socket = nullptr;
  }

  m_socket = new QLocalSocket(this);

  int waited = 0;
  while (waited < timeoutMs)
  {
    m_socket->connectToServer(m_socketPath);
    if (m_socket->waitForConnected(250))
    {
      qInfo() << "VlcBackend: RC socket connected";
      // Read the VLC greeting
      m_socket->waitForReadyRead(500);
      m_socket->readAll();
      return true;
    }
    QThread::msleep(100);
    waited += 350;
  }

  qWarning() << "VlcBackend: RC socket connection timed out";
  return false;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::disconnectSocket()
{
  if (m_socket)
  {
    m_socket->disconnectFromServer();
    m_socket->deleteLater();
    m_socket = nullptr;
  }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
QString VlcBackend::sendCommand(const QString& cmd, int timeoutMs)
{
  QMutexLocker lock(&m_socketMutex);

  if (!m_socket || m_socket->state() != QLocalSocket::ConnectedState)
    return QString();

  QByteArray data = (cmd + "\n").toUtf8();
  m_socket->write(data);
  m_socket->flush();

  if (!m_socket->waitForReadyRead(timeoutMs))
    return QString();

  QByteArray response = m_socket->readAll();
  return QString::fromUtf8(response).trimmed();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::pollStatus()
{
  if (!m_playing)
    return;

  // Get current time (seconds)
  QString timeStr = sendCommand("get_time", 200);
  bool ok = false;
  double timeSecs = timeStr.toDouble(&ok);
  if (ok)
  {
    qint64 newPos = static_cast<qint64>(timeSecs * 1000);
    if (newPos != m_position)
    {
      m_position = newPos;
      emit backendPositionChanged(m_position);
    }
  }

  // Get duration (seconds) — only query if unknown
  if (m_duration <= 0)
  {
    QString lenStr = sendCommand("get_length", 200);
    double lenSecs = lenStr.toDouble(&ok);
    if (ok && lenSecs > 0)
    {
      m_duration = static_cast<qint64>(lenSecs * 1000);
      emit backendDurationChanged(m_duration);
    }
  }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void VlcBackend::focusVlcWindow()
{
  // Use wlrctl to focus VLC's Wayland window (non-blocking, best-effort)
  QProcess::startDetached("wlrctl", {"toplevel", "focus", "app_id:vlc"});
}

///////////////////////////////////////////////////////////////////////////////////////////////////
qint64 VlcBackend::readMemAvailable() const
{
  QFile meminfo("/proc/meminfo");
  if (!meminfo.open(QIODevice::ReadOnly | QIODevice::Text))
    return 0;

  while (!meminfo.atEnd())
  {
    QByteArray line = meminfo.readLine();
    if (line.startsWith("MemAvailable:"))
    {
      // Format: "MemAvailable:   12345678 kB"
      QList<QByteArray> parts = line.simplified().split(' ');
      if (parts.size() >= 2)
        return parts[1].toLongLong() * 1024;  // kB -> bytes
    }
  }
  return 0;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
int VlcBackend::adaptiveNetworkCachingMs() const
{
  // network-caching controls startup delay (time before first frame).
  // Keep it LOW for fast startup. Deep read-ahead is via prefetch-buffer-size.
  return 1500;  // 1.5 seconds — fast first frame
}

///////////////////////////////////////////////////////////////////////////////////////////////////
int VlcBackend::adaptivePrefetchBytes() const
{
  qint64 available = readMemAvailable();
  if (available <= 0)
    return 16 * 1024 * 1024;  // 16MB fallback

  // Reserve 2GB for system, use 25% of remainder, cap at 512MB
  qint64 reserve = 2LL * 1024 * 1024 * 1024;
  qint64 usable = qMax(available - reserve, 256LL * 1024 * 1024);
  usable = qMin(usable, static_cast<qint64>(available * 0.75));

  qint64 prefetch = qMin(usable / 4, 512LL * 1024 * 1024);
  prefetch = qMax(prefetch, 1LL * 1024 * 1024);  // at least 1MB

  qInfo() << "VlcBackend: adaptive prefetch"
           << "available=" << (available / (1024 * 1024)) << "MB"
           << "prefetch=" << (prefetch / (1024 * 1024)) << "MB";

  return static_cast<int>(prefetch);
}
