#ifndef PLAYERCOMPONENT_H
#define PLAYERCOMPONENT_H

#include <QObject>
#include <QtCore/qglobal.h>
#include <QVariant>
#include <QSet>
#include <QQuickWindow>
#include <QQuickItem>
#include <QTimer>
#include <QTextStream>

#include <functional>

#include "ComponentManager.h"
#include "CodecsComponent.h"

#ifdef USE_VLC
#include <vlc/vlc.h>
#include <vlc/libvlc_media_player.h>
#else
#include "QtHelper.h"
#include <mpv/client.h>
#endif


///////////////////////////////////////////////////////////////////////////////////////////////////
class PlayerComponent : public ComponentBase
{
  Q_OBJECT
  DEFINE_SINGLETON(PlayerComponent);

public:
  const char* componentName() override { return "player"; }
  bool componentExport() override { return true; }
  bool componentInitialize() override;
  void componentPostInitialize() override;
  
  explicit PlayerComponent(QObject* parent = nullptr);
  ~PlayerComponent() override;

  Q_INVOKABLE bool load(const QString& url, const QVariantMap& options, const QVariantMap& metadata, const QString& audioStream = QString(), const QString& subtitleStream = QString());
  Q_INVOKABLE void queueMedia(const QString& url, const QVariantMap& options, const QVariantMap &metadata, const QString& audioStream, const QString& subtitleStream);
  Q_INVOKABLE void clearQueue();
  Q_INVOKABLE virtual void seekTo(qint64 ms);
  Q_INVOKABLE virtual void stop();
  Q_INVOKABLE virtual void streamSwitch();
  Q_INVOKABLE virtual void pause();
  Q_INVOKABLE virtual void play();
  Q_INVOKABLE virtual void setVolume(int volume);
  Q_INVOKABLE virtual int volume();
  Q_INVOKABLE virtual void setMuted(bool muted);
  Q_INVOKABLE virtual bool muted();
  Q_INVOKABLE virtual QVariant getAudioDeviceList();
  Q_INVOKABLE virtual void setAudioDevice(const QString& name);
  Q_INVOKABLE virtual void setAudioStream(const QString& audioStream);
  Q_INVOKABLE virtual void setSubtitleStream(const QString& subtitleStream);
  Q_INVOKABLE virtual void setAudioDelay(qint64 milliseconds);
  Q_INVOKABLE virtual void setSubtitleDelay(qint64 milliseconds);
  Q_INVOKABLE virtual void setVideoOnlyMode(bool enable);
  Q_INVOKABLE virtual bool checkCodecSupport(const QString& codec);
  Q_INVOKABLE virtual QList<CodecDriver> installedCodecDrivers();
  Q_INVOKABLE virtual QStringList installedDecoderCodecs();
  Q_INVOKABLE void userCommand(QString command);
  Q_INVOKABLE void setVideoRectangle(int x, int y, int w, int h);
  Q_INVOKABLE void setPlaybackRate(int rate);
  Q_INVOKABLE qint64 getPosition();
  Q_INVOKABLE qint64 getDuration();

  QRect videoRectangle() { return m_videoRectangle; }
#ifndef USE_VLC
  const mpv::qt::Handle getMpvHandle() const { return m_mpv; }
#endif
  virtual void setWindow(QQuickWindow* window);
  QString videoInformation() const;
  static QStringList AudioCodecsAll() { return { "ac3", "dts", "eac3", "dts-hd", "truehd" }; };
  static QStringList AudioCodecsSPDIF() { return { "ac3", "dts" }; };

  enum class State { finished, canceled, error, paused, playing, buffering };
  enum class MediaType { Subtitle, Audio };
  
public Q_SLOTS:
  void updateAudioDeviceList();
  void setAudioConfiguration();
  void setSubtitleConfiguration();
  void setVideoConfiguration();
  void setOtherConfiguration();
  void updateAudioConfiguration();
  void updateSubtitleConfiguration();
  void updateVideoConfiguration();
  void updateConfiguration();

private Q_SLOTS:
#ifdef USE_VLC
  void onPositionPoll();
#else
  void handleMpvEvents();
#endif
  void onRestoreDisplay();
  void onRefreshRateChange();
#ifndef USE_VLC
  void onCodecsLoadingDone(CodecsFetcher* sender);
  void updateAudioDevice();
#endif

Q_SIGNALS:
  void playing();
  void buffering(float percent);
  void paused();
  void finished();
  void canceled();
  void error(const QString& msg);
  void stopped();
  void stateChanged(State newState, State oldState);
  void videoPlaybackActive(bool active);
  void windowVisible(bool visible);
  void updateDuration(qint64 milliseconds);
  void positionUpdate(quint64);
  void onVideoRecangleChanged();
  void onMpvEvents();
  void onMetaData(const QVariantMap &meta, QUrl baseUrl);
  
private:
  void loadWithOptions(const QVariantMap& options);
  void setQtQuickWindow(QQuickWindow* window);
  void updatePlaybackState();
#ifdef USE_VLC
  static void vlcEventCallback(const libvlc_event_t* event, void* userData);
  void writeForegroundApp(const QString& app);
#else
  void handleMpvEvent(mpv_event *event);
#endif
  bool switchDisplayFrameRate();
  void checkCurrentAudioDevice(const QSet<QString>& old_devs, const QSet<QString>& new_devs);
  void appendAudioFormat(QTextStream& info, const QString& property) const;
  void initializeCodecSupport();
  PlaybackInfo getPlaybackInfo();
  void setPreferredCodecs(const QList<CodecDriver>& codecs);
  void startCodecsLoading(std::function<void()> resume);
  void updateVideoAspectSettings();
  QVariantList findStreamsForURL(const QString &url);
  void reselectStream(const QString &streamSelection, MediaType target);

#ifdef USE_VLC
  libvlc_instance_t* m_vlcInstance = nullptr;
  libvlc_media_player_t* m_vlcPlayer = nullptr;
  QTimer* m_positionTimer = nullptr;
  int m_currentVolume = 100;
  bool m_isMuted = false;
  bool m_durationEmitted = false;
#else
  mpv::qt::Handle m_mpv;
#endif
  State m_state;
  bool m_paused;
  bool m_playbackActive;
  bool m_windowVisible;
  bool m_videoPlaybackActive;
  bool m_inPlayback;
  bool m_playbackCanceled;
  QString m_playbackError;
  int m_bufferingPercentage;
  int m_lastBufferingPercentage;
  double m_lastPositionUpdate;
  qint64 m_playbackAudioDelay;
  QQuickWindow* m_window;
  float m_mediaFrameRate;
  QTimer m_restoreDisplayTimer;
  QTimer m_reloadAudioTimer;
  QSet<QString> m_audioDevices;
  bool m_streamSwitchImminent;
  QMap<QString, bool> m_codecSupport;
  bool m_doAc3Transcoding;
  QStringList m_passthroughCodecs;
  QVariantMap m_serverMediaInfo;
  QString m_currentSubtitleStream;
  QString m_currentAudioStream;
  QRect m_videoRectangle;
};

#endif // PLAYERCOMPONENT_H
