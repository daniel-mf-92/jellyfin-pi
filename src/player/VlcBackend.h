#ifndef VLCBACKEND_H
#define VLCBACKEND_H

#include "PlayerBackend.h"

#include <QProcess>
#include <QLocalSocket>
#include <QTimer>
#include <QMutex>

///////////////////////////////////////////////////////////////////////////////////////////////////
/// VLC backend — runs cvlc as a subprocess, controlled via RC Unix socket.
///
/// Video output: VLC opens its own fullscreen Wayland window (separate from Qt).
/// Adaptive cache: reads /proc/meminfo and sizes --prefetch-buffer-size accordingly.
/// Splash: caller hides the web view before play() → user sees black → VLC appears on top.
///
class VlcBackend : public PlayerBackend
{
  Q_OBJECT

public:
  explicit VlcBackend(QObject* parent = nullptr);
  ~VlcBackend() override;

  QString name() const override { return QStringLiteral("vlc"); }

  // Lifecycle
  bool initialize() override;
  void cleanup() override;

  // Playback
  void play(const QString& url, const QVariantMap& options) override;
  void stop() override;
  void pause() override;
  void unpause() override;
  void seekTo(qint64 ms) override;

  // Audio
  void setVolume(int volume) override;
  int volume() override;
  void setMuted(bool muted) override;
  bool muted() override;
  void setAudioTrack(int trackId) override;
  void setAudioDelay(qint64 ms) override;
  void setAudioDevice(const QString& name) override;
  QVariant getAudioDeviceList() override;

  // Video
  void setVideoRectangle(int x, int y, int w, int h) override;

  // Subtitles
  void setSubtitleTrack(int trackId) override;
  void setSubtitleDelay(qint64 ms) override;
  void addSubtitleFile(const QString& path) override;

  // Playback rate
  void setRate(double rate) override;

  // Query
  qint64 getPosition() override;
  qint64 getDuration() override;
  bool isPlaying() const override;

  // Configuration
  void updateAudioConfiguration() override;
  void updateVideoConfiguration() override;

  // Debug
  QString videoInformation() const override;

private Q_SLOTS:
  void onProcessFinished(int exitCode, QProcess::ExitStatus status);
  void onProcessStarted();
  void pollStatus();

private:
  void launchVlc(const QString& url, double startTimeSecs);
  void killVlc();
  bool connectSocket(int timeoutMs = 5000);
  void disconnectSocket();
  QString sendCommand(const QString& cmd, int timeoutMs = 500);
  void focusVlcWindow();
  int adaptiveNetworkCachingMs() const;
  int adaptivePrefetchBytes() const;
  qint64 readMemAvailable() const;

  QProcess* m_process = nullptr;
  QLocalSocket* m_socket = nullptr;
  QString m_socketPath;
  QTimer* m_pollTimer = nullptr;

  qint64 m_position = 0;     // ms
  qint64 m_duration = 0;     // ms
  int m_volume = 100;         // 0-100
  bool m_muted = false;
  bool m_playing = false;
  bool m_paused = false;
  bool m_waitingForWindow = false;

  QMutex m_socketMutex;
};

#endif // VLCBACKEND_H
