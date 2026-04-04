#ifndef PLAYERBACKEND_H
#define PLAYERBACKEND_H

#include <QObject>
#include <QString>
#include <QVariantMap>

///////////////////////////////////////////////////////////////////////////////////////////////////
/// Abstract interface for media player backends (mpv, VLC, etc.)
///
/// PlayerComponent delegates all playback operations to the active backend.
/// Each backend implements this interface with its own engine.
///
class PlayerBackend : public QObject
{
  Q_OBJECT

public:
  explicit PlayerBackend(QObject* parent = nullptr) : QObject(parent) {}
  virtual ~PlayerBackend() = default;

  virtual QString name() const = 0;

  // Lifecycle
  virtual bool initialize() = 0;
  virtual void cleanup() = 0;

  // Playback
  virtual void play(const QString& url, const QVariantMap& options) = 0;
  virtual void stop() = 0;
  virtual void pause() = 0;
  virtual void unpause() = 0;
  virtual void seekTo(qint64 ms) = 0;

  // Audio
  virtual void setVolume(int volume) = 0;     // 0-100
  virtual int volume() = 0;
  virtual void setMuted(bool muted) = 0;
  virtual bool muted() = 0;
  virtual void setAudioTrack(int trackId) = 0;
  virtual void setAudioDelay(qint64 ms) = 0;
  virtual void setAudioDevice(const QString& name) = 0;
  virtual QVariant getAudioDeviceList() = 0;

  // Video
  virtual void setVideoRectangle(int x, int y, int w, int h) { Q_UNUSED(x); Q_UNUSED(y); Q_UNUSED(w); Q_UNUSED(h); }

  // Subtitles
  virtual void setSubtitleTrack(int trackId) = 0;
  virtual void setSubtitleDelay(qint64 ms) = 0;
  virtual void addSubtitleFile(const QString& path) { Q_UNUSED(path); }

  // Playback rate
  virtual void setRate(double rate) = 0;

  // Query
  virtual qint64 getPosition() = 0;   // ms
  virtual qint64 getDuration() = 0;   // ms
  virtual bool isPlaying() const = 0;

  // Configuration (called when settings change)
  virtual void updateAudioConfiguration() {}
  virtual void updateVideoConfiguration() {}
  virtual void updateSubtitleConfiguration() {}

  // Debug
  virtual QString videoInformation() const { return QString(); }

Q_SIGNALS:
  void backendPlaying();
  void backendPaused();
  void backendFinished();
  void backendCanceled();
  void backendError(const QString& msg);
  void backendBuffering(int percent);
  void backendPositionChanged(qint64 ms);
  void backendDurationChanged(qint64 ms);
  void backendVideoPlaybackActive(bool active);
  void backendBufferedRangesUpdated(const QVariantList& ranges);
};

#endif // PLAYERBACKEND_H
