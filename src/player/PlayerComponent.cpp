#include "PlayerComponent.h"
#include <QString>
#include <Qt>
#include <QDir>
#include <QCoreApplication>
#include <QGuiApplication>
#include <QDebug>
#include "display/DisplayComponent.h"
#include "settings/SettingsComponent.h"
#include "system/SystemComponent.h"
#include "utils/Utils.h"
#include "utils/Log.h"
#include "ComponentManager.h"
#include "settings/SettingsSection.h"

#include "input/InputComponent.h"

#include <math.h>
#include <string.h>
#include <shared/Paths.h>
#include <QRegularExpression>
#include <QFile>
#include <QMetaObject>

#if !defined(Q_OS_WIN)
#include <unistd.h>
#endif

#include <QProcess>
#include <cstdlib>

///////////////////////////////////////////////////////////////////////////////////////////////////
// Static VLC event callback — posts events to the Qt event loop via QMetaObject::invokeMethod
///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::vlcEventCallback(const libvlc_event_t* event, void* userData)
{
  PlayerComponent* self = static_cast<PlayerComponent*>(userData);

  switch (event->type)
  {
    case libvlc_MediaPlayerPlaying:
      QMetaObject::invokeMethod(self, [self]() {
        qInfo() << "VLC: MediaPlayerPlaying";
        self->m_inPlayback = true;
        self->m_paused = false;
        self->m_playbackActive = true;
        self->m_windowVisible = true;
        self->writeForegroundApp("vlc");
        self->updatePlaybackState();

        // Emit duration once we start playing
        if (self->m_vlcPlayer && !self->m_durationEmitted) {
          libvlc_time_t dur = libvlc_media_player_get_length(self->m_vlcPlayer);
          if (dur > 0) {
            self->m_durationEmitted = true;
            emit self->updateDuration(static_cast<qint64>(dur));
          }
        }
      }, Qt::QueuedConnection);
      break;

    case libvlc_MediaPlayerPaused:
      QMetaObject::invokeMethod(self, [self]() {
        qInfo() << "VLC: MediaPlayerPaused";
        self->m_paused = true;
        self->m_playbackActive = false;
        self->updatePlaybackState();
      }, Qt::QueuedConnection);
      break;

    case libvlc_MediaPlayerStopped:
      QMetaObject::invokeMethod(self, [self]() {
        qInfo() << "VLC: MediaPlayerStopped";
        self->m_inPlayback = false;
        self->m_playbackActive = false;
        self->m_windowVisible = false;
        self->m_playbackCanceled = true;
        self->m_durationEmitted = false;
        self->writeForegroundApp("jellyfin");
        self->updatePlaybackState();
      }, Qt::QueuedConnection);
      break;

    case libvlc_MediaPlayerEndReached:
      QMetaObject::invokeMethod(self, [self]() {
        qInfo() << "VLC: MediaPlayerEndReached";
        self->m_inPlayback = false;
        self->m_playbackActive = false;
        self->m_windowVisible = false;
        self->m_playbackCanceled = false;
        self->m_playbackError = "";
        self->m_durationEmitted = false;
        self->writeForegroundApp("jellyfin");
        self->updatePlaybackState();
      }, Qt::QueuedConnection);
      break;

    case libvlc_MediaPlayerEncounteredError:
      QMetaObject::invokeMethod(self, [self]() {
        qWarning() << "VLC: MediaPlayerEncounteredError";
        self->m_inPlayback = false;
        self->m_playbackActive = false;
        self->m_playbackError = "VLC playback error";
        self->m_durationEmitted = false;
        self->writeForegroundApp("jellyfin");
        self->updatePlaybackState();
      }, Qt::QueuedConnection);
      break;

    case libvlc_MediaPlayerBuffering:
      QMetaObject::invokeMethod(self, [self, percent = event->u.media_player_buffering.new_cache]() {
        self->m_bufferingPercentage = static_cast<int>(percent);
        if (percent >= 100.0f) {
          self->m_playbackActive = true;
        }
        self->updatePlaybackState();
      }, Qt::QueuedConnection);
      break;

    case libvlc_MediaPlayerLengthChanged:
      QMetaObject::invokeMethod(self, [self, newLength = event->u.media_player_length_changed.new_length]() {
        if (newLength > 0) {
          self->m_durationEmitted = true;
          emit self->updateDuration(static_cast<qint64>(newLength));
        }
      }, Qt::QueuedConnection);
      break;

    default:
      break;
  }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
PlayerComponent::PlayerComponent(QObject* parent)
  : ComponentBase(parent), m_state(State::finished), m_paused(false), m_playbackActive(false),
  m_windowVisible(false), m_videoPlaybackActive(false), m_inPlayback(false), m_playbackCanceled(false),
  m_bufferingPercentage(100), m_lastBufferingPercentage(-1),
  m_lastPositionUpdate(0.0), m_playbackAudioDelay(0),
  m_window(nullptr), m_mediaFrameRate(0),
  m_restoreDisplayTimer(this),
  m_streamSwitchImminent(false), m_doAc3Transcoding(false),
  m_videoRectangle(-1, 0, 0, 0)
{
  m_restoreDisplayTimer.setSingleShot(true);
  connect(&m_restoreDisplayTimer, &QTimer::timeout, this, &PlayerComponent::onRestoreDisplay);

  connect(&DisplayComponent::Get(), &DisplayComponent::refreshRateChanged, this, &PlayerComponent::onRefreshRateChange);
}

/////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::componentPostInitialize()
{
  InputComponent::Get().registerHostCommand("player", this, "userCommand");
}

///////////////////////////////////////////////////////////////////////////////////////////////////
PlayerComponent::~PlayerComponent()
{
  if (m_positionTimer) {
    m_positionTimer->stop();
    delete m_positionTimer;
    m_positionTimer = nullptr;
  }

  if (m_vlcPlayer) {
    libvlc_media_player_stop(m_vlcPlayer);
    libvlc_media_player_release(m_vlcPlayer);
    m_vlcPlayer = nullptr;
  }

  if (m_vlcInstance) {
    libvlc_release(m_vlcInstance);
    m_vlcInstance = nullptr;
  }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
bool PlayerComponent::componentInitialize()
{
  qInfo() << "PlayerComponent::componentInitialize - creating libvlc instance";

  // VLC command line arguments for the instance
  const char* vlcArgs[] = {
    "--fullscreen",
    "--no-video-title-show",
    "--avcodec-hw=any",
    "--audio-desync=-300",
    "--no-osd",
    "--file-caching=3000",
    "--network-caching=5000",
    "--no-xlib",
    "--verbose=2"
  };

  m_vlcInstance = libvlc_new(sizeof(vlcArgs) / sizeof(vlcArgs[0]), vlcArgs);
  if (!m_vlcInstance) {
    qCritical() << "Failed to create libvlc instance:" << libvlc_errmsg();
    return false;
  }

  m_vlcPlayer = libvlc_media_player_new(m_vlcInstance);
  if (!m_vlcPlayer) {
    qCritical() << "Failed to create libvlc media player:" << libvlc_errmsg();
    libvlc_release(m_vlcInstance);
    m_vlcInstance = nullptr;
    return false;
  }

  // Attach VLC event callbacks
  libvlc_event_manager_t* em = libvlc_media_player_event_manager(m_vlcPlayer);
  libvlc_event_attach(em, libvlc_MediaPlayerPlaying, vlcEventCallback, this);
  libvlc_event_attach(em, libvlc_MediaPlayerPaused, vlcEventCallback, this);
  libvlc_event_attach(em, libvlc_MediaPlayerStopped, vlcEventCallback, this);
  libvlc_event_attach(em, libvlc_MediaPlayerEndReached, vlcEventCallback, this);
  libvlc_event_attach(em, libvlc_MediaPlayerEncounteredError, vlcEventCallback, this);
  libvlc_event_attach(em, libvlc_MediaPlayerBuffering, vlcEventCallback, this);
  libvlc_event_attach(em, libvlc_MediaPlayerLengthChanged, vlcEventCallback, this);

  // Set VLC to fullscreen mode
  libvlc_set_fullscreen(m_vlcPlayer, 1);

  // Set initial volume
  libvlc_audio_set_volume(m_vlcPlayer, m_currentVolume);

  // Setup position polling timer (replaces mpv property observation)
  m_positionTimer = new QTimer(this);
  m_positionTimer->setInterval(500); // Poll every 500ms
  connect(m_positionTimer, &QTimer::timeout, this, &PlayerComponent::onPositionPoll);

  qInfo() << "VLC player initialized successfully";
  return true;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::writeForegroundApp(const QString& app)
{
  QFile f("/tmp/foreground-app");
  if (f.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
    f.write(app.toUtf8());
    f.close();
  }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::onPositionPoll()
{
  if (!m_vlcPlayer || !m_inPlayback)
    return;

  libvlc_time_t posMs = libvlc_media_player_get_time(m_vlcPlayer);
  if (posMs >= 0) {
    double posSec = posMs / 1000.0;
    if (fabs(posSec - m_lastPositionUpdate) > 0.015) {
      quint64 ms = static_cast<quint64>(qMax(static_cast<qint64>(posMs), static_cast<qint64>(0)));
      emit positionUpdate(ms);
      m_lastPositionUpdate = posSec;
    }
  }

  // Also check duration if not emitted yet
  if (!m_durationEmitted) {
    libvlc_time_t dur = libvlc_media_player_get_length(m_vlcPlayer);
    if (dur > 0) {
      m_durationEmitted = true;
      emit updateDuration(static_cast<qint64>(dur));
    }
  }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::setVideoRectangle(int x, int y, int w, int h)
{
  QRect rc(x, y, w, h);
  if (rc != m_videoRectangle)
  {
    m_videoRectangle = rc;
    emit onVideoRecangleChanged();
  }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::setWindow(QQuickWindow* window)
{
  m_window = window;
  // VLC creates its own fullscreen window, no Qt embedding needed
}

///////////////////////////////////////////////////////////////////////////////////////////////////
bool PlayerComponent::load(const QString& url, const QVariantMap& options, const QVariantMap &metadata, const QString& audioStream, const QString& subtitleStream)
{
  stop();
  queueMedia(url, options, metadata, audioStream, subtitleStream);
  return true;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::queueMedia(const QString& url, const QVariantMap& options, const QVariantMap &metadata, const QString& audioStream, const QString& subtitleStream)
{
  if (!m_vlcInstance || !m_vlcPlayer) {
    qWarning() << "PlayerComponent::queueMedia: VLC not initialized yet";
    return;
  }

  InputComponent::Get().cancelAutoRepeat();

  m_mediaFrameRate = metadata["frameRate"].toFloat();
  m_serverMediaInfo = metadata["media"].toMap();
  m_currentSubtitleStream = subtitleStream;
  m_currentAudioStream = audioStream;
  m_durationEmitted = false;

  QUrl qurl = url;
  QByteArray urlBytes = qurl.toString(QUrl::FullyEncoded).toUtf8();

  qInfo() << "VLC: Loading media URL:" << qurl.toString(QUrl::FullyEncoded);

  // Create media from URL
  libvlc_media_t* media = libvlc_media_new_location(m_vlcInstance, urlBytes.constData());
  if (!media) {
    qCritical() << "VLC: Failed to create media from URL:" << libvlc_errmsg();
    m_playbackError = "Failed to create VLC media";
    updatePlaybackState();
    return;
  }

  // Set start time if specified
  quint64 startMilliseconds = options["startMilliseconds"].toLongLong();
  if (startMilliseconds > 0) {
    QString startOption = QString(":start-time=%1").arg(startMilliseconds / 1000.0, 0, 'f', 3);
    libvlc_media_add_option(media, startOption.toUtf8().constData());
    qInfo() << "VLC: Setting start time:" << startOption;
  }

  // Set autoplay behavior
  bool autoplay = options["autoplay"].toBool();

  // Set audio stream if specified
  if (!audioStream.isEmpty()) {
    bool aOk; int trackId = audioStream.toInt(&aOk);
    if (trackId >= 0) {
      QString audioOption = QString(":audio-track=%1").arg(trackId);
      libvlc_media_add_option(media, audioOption.toUtf8().constData());
    }
  }

  // Set subtitle stream if specified
  if (!subtitleStream.isEmpty()) {
    bool sOk; int trackId = subtitleStream.toInt(&sOk);
    if (trackId >= 0) {
      QString subOption = QString(":sub-track=%1").arg(trackId);
      libvlc_media_add_option(media, subOption.toUtf8().constData());
    } else {
      libvlc_media_add_option(media, ":no-sub-autodetect-file");
    }
  }

  // If music, disable video
  if (metadata["type"] == "music") {
    libvlc_media_add_option(media, ":no-video");
  }

  // Set user agent if provided
  QString userAgent = metadata["headers"].toMap()["User-Agent"].toString();
  if (!userAgent.isEmpty()) {
    QString uaOption = QString(":http-user-agent=%1").arg(userAgent);
    libvlc_media_add_option(media, uaOption.toUtf8().constData());
  }

  // Set the media on the player
  libvlc_media_player_set_media(m_vlcPlayer, media);
  libvlc_media_release(media); // player holds its own reference

  // Start playback
  m_playbackCanceled = false;
  m_playbackError = "";
  m_inPlayback = true;

  if (libvlc_media_player_play(m_vlcPlayer) != 0) {
    qCritical() << "VLC: Failed to start playback:" << libvlc_errmsg();
    m_playbackError = "VLC failed to start playback";
    m_inPlayback = false;
    updatePlaybackState();
    return;
  }

  // If not autoplay, pause immediately after starting
  if (!autoplay) {
    // Small delay to let VLC start, then pause
    QTimer::singleShot(100, this, [this]() {
      if (m_vlcPlayer && m_inPlayback) {
        libvlc_media_player_set_pause(m_vlcPlayer, 1);
      }
    });
  }

  // Start position polling
  m_positionTimer->start();

  // Emit metadata
  QVariantMap jellyfinMetadata = metadata["metadata"].toMap();
  QUrl jellyfinBaseUrl = qurl.adjusted(QUrl::RemovePath | QUrl::RemoveQuery);
  emit onMetaData(jellyfinMetadata, jellyfinBaseUrl);
}

/////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::streamSwitch()
{
  m_streamSwitchImminent = true;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::onRestoreDisplay()
{
  if (!m_inPlayback)
    DisplayComponent::Get().restorePreviousVideoMode();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::onRefreshRateChange()
{
  // Nothing specific needed for VLC — it manages its own display
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::updatePlaybackState()
{
  State newState = m_state;

  if (m_inPlayback) {
    if (m_paused) {
      newState = State::paused;
    } else if (m_playbackActive) {
      newState = State::playing;
    } else {
      if (m_bufferingPercentage == 100)
        m_bufferingPercentage = 0;
      newState = State::buffering;
    }
  } else {
    if (!m_playbackError.isEmpty())
      newState = State::error;
    else if (m_playbackCanceled)
      newState = State::canceled;
    else
      newState = State::finished;
  }

  if (newState != m_state)
  {
    switch (newState) {
    case State::paused:
      qInfo() << "Entering state: paused";
      emit paused();
      break;
    case State::playing:
      qInfo() << "Entering state: playing";
      emit playing();
      break;
    case State::buffering:
      qInfo() << "Entering state: buffering";
      m_lastBufferingPercentage = -1;
      break;
    case State::finished:
      qInfo() << "Entering state: finished";
      m_positionTimer->stop();
      emit finished();
      emit stopped();
      break;
    case State::canceled:
      qInfo() << "Entering state: canceled";
      m_positionTimer->stop();
      emit canceled();
      emit stopped();
      break;
    case State::error:
      qInfo() << ("Entering state: error (" + m_playbackError + ")");
      m_positionTimer->stop();
      emit error(m_playbackError);
      break;
    }
    emit stateChanged(newState, m_state);
    m_state = newState;
  }

  if (m_state == State::buffering && m_lastBufferingPercentage != m_bufferingPercentage)
    emit buffering(m_bufferingPercentage);
  m_lastBufferingPercentage = m_bufferingPercentage;

  bool is_videoPlaybackActive = m_state == State::playing && m_windowVisible;
  if (m_videoPlaybackActive != is_videoPlaybackActive)
  {
    m_videoPlaybackActive = is_videoPlaybackActive;
    emit videoPlaybackActive(m_videoPlaybackActive);
  }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::setVideoOnlyMode(bool enable)
{
  if (m_window)
  {
    QQuickItem *web = m_window->findChild<QQuickItem *>("web");
    if (web)
      web->setVisible(!enable);
  }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::play()
{
  if (!m_vlcPlayer) {
    qWarning() << "PlayerComponent::play: VLC not initialized yet";
    return;
  }
  libvlc_media_player_set_pause(m_vlcPlayer, 0);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::stop()
{
  if (!m_vlcPlayer) {
    qWarning() << "PlayerComponent::stop: VLC not initialized yet";
    return;
  }
  qInfo() << "VLC: Stopping playback";
  m_positionTimer->stop();
  libvlc_media_player_stop(m_vlcPlayer);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::clearQueue()
{
  // VLC does not have a built-in playlist queue in libvlc — this is a no-op.
  // The web client manages the queue; we only play one item at a time.
  qInfo() << "VLC: clearQueue (no-op, web client manages queue)";
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::pause()
{
  if (!m_vlcPlayer) {
    qWarning() << "PlayerComponent::pause: VLC not initialized yet";
    return;
  }
  libvlc_media_player_set_pause(m_vlcPlayer, 1);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::seekTo(qint64 ms)
{
  if (!m_vlcPlayer) {
    qWarning() << "PlayerComponent::seekTo: VLC not initialized yet";
    return;
  }
  qInfo() << "VLC: Seeking to" << ms << "ms";
  libvlc_media_player_set_time(m_vlcPlayer, ms);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
QVariant PlayerComponent::getAudioDeviceList()
{
  if (!m_vlcPlayer) {
    qWarning() << "PlayerComponent::getAudioDeviceList: VLC not initialized yet";
    return QVariant();
  }

  QVariantList devices;
  libvlc_audio_output_device_t* devList = libvlc_audio_output_device_enum(m_vlcPlayer);
  libvlc_audio_output_device_t* dev = devList;
  while (dev) {
    QVariantMap entry;
    entry["name"] = QString::fromUtf8(dev->psz_device);
    entry["description"] = QString::fromUtf8(dev->psz_description);
    devices.append(entry);
    dev = dev->p_next;
  }
  if (devList)
    libvlc_audio_output_device_list_release(devList);

  return devices;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::setAudioDevice(const QString& name)
{
  if (!m_vlcPlayer) {
    qWarning() << "PlayerComponent::setAudioDevice: VLC not initialized yet";
    return;
  }
  libvlc_audio_output_device_set(m_vlcPlayer, nullptr, name.toUtf8().constData());
  qInfo() << "VLC: Set audio device to:" << name;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::setVolume(int vol)
{
  if (!m_vlcPlayer) {
    qWarning() << "PlayerComponent::setVolume: VLC not initialized yet";
    return;
  }
  m_currentVolume = vol;
  libvlc_audio_set_volume(m_vlcPlayer, vol);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
int PlayerComponent::volume()
{
  if (!m_vlcPlayer) {
    return 0;
  }
  return libvlc_audio_get_volume(m_vlcPlayer);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::setMuted(bool muted)
{
  if (!m_vlcPlayer) {
    qWarning() << "PlayerComponent::setMuted: VLC not initialized yet";
    return;
  }
  m_isMuted = muted;
  libvlc_audio_set_mute(m_vlcPlayer, muted ? 1 : 0);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
bool PlayerComponent::muted()
{
  if (!m_vlcPlayer) {
    return false;
  }
  return libvlc_audio_get_mute(m_vlcPlayer) != 0;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::setAudioStream(const QString &audioStream)
{
  if (!m_vlcPlayer) {
    qWarning() << "PlayerComponent::setAudioStream: VLC not initialized yet";
    return;
  }
  m_currentAudioStream = audioStream;

  if (!audioStream.isEmpty()) {
    bool ok;
    int trackId = audioStream.toInt(&ok);
    if (ok && trackId >= 0) {
      libvlc_audio_set_track(m_vlcPlayer, trackId);
      qInfo() << "VLC: Set audio track to:" << trackId;
    }
  }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::setSubtitleStream(const QString &subtitleStream)
{
  if (!m_vlcPlayer) {
    qWarning() << "PlayerComponent::setSubtitleStream: VLC not initialized yet";
    return;
  }
  m_currentSubtitleStream = subtitleStream;

  if (!subtitleStream.isEmpty()) {
    bool ok;
    int trackId = subtitleStream.toInt(&ok);
    if (ok) {
      if (trackId < 0) {
        // Disable subtitles
        libvlc_video_set_spu(m_vlcPlayer, -1);
        qInfo() << "VLC: Subtitles disabled";
      } else {
        libvlc_video_set_spu(m_vlcPlayer, trackId);
        qInfo() << "VLC: Set subtitle track to:" << trackId;
      }
    } else {
      // Handle external subtitle URL
      if (subtitleStream.startsWith("#")) {
        qsizetype splitPos = subtitleStream.indexOf(",");
        if (splitPos > 0) {
          QString subUrl = subtitleStream.mid(splitPos + 1);
          if (!subUrl.isEmpty()) {
            libvlc_media_player_add_slave(m_vlcPlayer, libvlc_media_slave_type_subtitle,
                                           subUrl.toUtf8().constData(), true);
            qInfo() << "VLC: Added external subtitle:" << subUrl;
          }
        }
      }
    }
  }
}

/////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::setAudioDelay(qint64 milliseconds)
{
  if (!m_vlcPlayer) {
    qWarning() << "PlayerComponent::setAudioDelay: VLC not initialized yet";
    return;
  }
  m_playbackAudioDelay = milliseconds;
  // VLC audio delay is in microseconds
  libvlc_audio_set_delay(m_vlcPlayer, milliseconds * 1000);
  qInfo() << "VLC: Set audio delay to:" << milliseconds << "ms";
}

/////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::setSubtitleDelay(qint64 milliseconds)
{
  if (!m_vlcPlayer) {
    qWarning() << "PlayerComponent::setSubtitleDelay: VLC not initialized yet";
    return;
  }
  // VLC subtitle delay is in microseconds
  libvlc_video_set_spu_delay(m_vlcPlayer, milliseconds * 1000);
  qInfo() << "VLC: Set subtitle delay to:" << milliseconds << "ms";
}

/////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::setPlaybackRate(int rate)
{
  if (!m_vlcPlayer) {
    qWarning() << "PlayerComponent::setPlaybackRate: VLC not initialized yet";
    return;
  }
  float speed = rate / 1000.0f;
  libvlc_media_player_set_rate(m_vlcPlayer, speed);
  qInfo() << "VLC: Set playback rate to:" << speed;
}

/////////////////////////////////////////////////////////////////////////////////////////
qint64 PlayerComponent::getPosition()
{
  if (!m_vlcPlayer) {
    return 0;
  }
  return static_cast<qint64>(libvlc_media_player_get_time(m_vlcPlayer));
}

/////////////////////////////////////////////////////////////////////////////////////////
qint64 PlayerComponent::getDuration()
{
  if (!m_vlcPlayer) {
    return 0;
  }
  return static_cast<qint64>(libvlc_media_player_get_length(m_vlcPlayer));
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::updateAudioDeviceList()
{
  // VLC manages audio devices internally. We just report what is available.
  QVariantList settingList;
  QVariant list = getAudioDeviceList();
  for (const QVariant& d : list.toList()) {
    QVariantMap dmap = d.toMap();
    QVariantMap entry;
    entry["value"] = dmap["name"];
    entry["title"] = dmap["description"];
    settingList << entry;
  }
  SettingsComponent::Get().updatePossibleValues(SETTINGS_SECTION_AUDIO, "device", settingList);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::updateAudioConfiguration()
{
  setAudioConfiguration();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::setAudioConfiguration()
{
  // VLC audio configuration is mostly handled by VLC instance args.
  // We update the audio device if the user changed it in settings.
  QString device = SettingsComponent::Get().value(SETTINGS_SECTION_AUDIO, "device").toString();
  if (!device.isEmpty() && device != "auto") {
    setAudioDevice(device);
  }
  qInfo() << "VLC: Audio configuration updated (device:" << device << ")";
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::updateSubtitleConfiguration()
{
  setSubtitleConfiguration();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::setSubtitleConfiguration()
{
  // VLC subtitle configuration — most settings are handled per-media.
  qInfo() << "VLC: Subtitle configuration updated (VLC uses built-in subtitle rendering)";
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::updateVideoConfiguration()
{
  setVideoConfiguration();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::setVideoConfiguration()
{
  // VLC video configuration is handled by instance creation args.
  qInfo() << "VLC: Video configuration updated (static, set at instance creation)";
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::setOtherConfiguration()
{
  // No-op for VLC — mpv-specific settings not applicable.
  qInfo() << "VLC: Other configuration (no-op)";
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::updateConfiguration()
{
  setAudioConfiguration();
  setVideoConfiguration();
  setSubtitleConfiguration();
  setOtherConfiguration();
}

/////////////////////////////////////////////////////////////////////////////////////////
void PlayerComponent::userCommand(QString command)
{
  qWarning() << "VLC: userCommand not supported (mpv command string):" << command;
}

/////////////////////////////////////////////////////////////////////////////////////////
QString PlayerComponent::videoInformation() const
{
  if (!m_vlcPlayer)
    return "";

  if (!m_inPlayback)
    return "";

  QString infoStr;
  QTextStream info(&infoStr);

  info << "VLC Backend\n\n";

  info << "File:\n";
  libvlc_media_t* media = libvlc_media_player_get_media(m_vlcPlayer);
  if (media) {
    char* mrl = libvlc_media_get_mrl(media);
    if (mrl) {
      QString mrlStr = QString::fromUtf8(mrl);
      Log::CensorAuthTokens(mrlStr);
      info << "URL: " << mrlStr << "\n";
      libvlc_free(mrl);
    }
  }
  info << "\n";

  info << "Video:\n";
  unsigned int vw = 0, vh = 0;
  if (libvlc_video_get_size(m_vlcPlayer, 0, &vw, &vh) == 0) {
    info << "Size: " << vw << "x" << vh << "\n";
  }
  info << "Hardware Decoding: avcodec-hw=any (requested)\n";
  info << "\n";

  info << "Audio:\n";
  int audioTrack = libvlc_audio_get_track(m_vlcPlayer);
  info << "Current audio track: " << audioTrack << "\n";
  info << "Volume: " << libvlc_audio_get_volume(m_vlcPlayer) << "\n";
  info << "\n";

  info << "Playback:\n";
  libvlc_time_t pos = libvlc_media_player_get_time(m_vlcPlayer);
  libvlc_time_t dur = libvlc_media_player_get_length(m_vlcPlayer);
  info << "Time: " << (pos / 1000.0) << "s / " << (dur / 1000.0) << "s\n";
  float rate = libvlc_media_player_get_rate(m_vlcPlayer);
  info << "Rate: " << rate << "\n";

  info.flush();
  return infoStr;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
bool PlayerComponent::checkCodecSupport(const QString& codec)
{
  // VLC handles all codec support internally - always report supported
  Q_UNUSED(codec);
  return true;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
QList<CodecDriver> PlayerComponent::installedCodecDrivers()
{
  // VLC handles codecs internally - return empty list
  return QList<CodecDriver>();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
QStringList PlayerComponent::installedDecoderCodecs()
{
  // VLC handles codecs internally - return empty list
  return QStringList();
}
