#ifndef STREAMCACHEMANAGER_H
#define STREAMCACHEMANAGER_H

#include <QObject>
#include <QMap>
#include <QMutex>
#include <QTimer>
#include <QThread>

///////////////////////////////////////////////////////////////////////////////////////////////////
/// /dev/shm LRU media cache — downloads streams to RAM-backed tmpfs.
///
/// After playback, cached data stays in RAM. On replay, serves from file://.
/// Under memory pressure (MemAvailable < floor), evicts oldest-accessed entry.
/// Currently-playing entry is pinned (never evicted).
///
class StreamCacheManager : public QObject
{
  Q_OBJECT

public:
  explicit StreamCacheManager(QObject* parent = nullptr);
  ~StreamCacheManager() override;

  /// Return file:// path if item is fully cached, else empty string.
  QString getCached(const QString& itemId);

  /// Start background download to /dev/shm (idempotent).
  void startDownload(const QString& itemId, const QString& url);

  /// Cancel an in-progress download.
  void cancelDownload(const QString& itemId);

  /// Pin item as currently playing (immune to eviction).
  void pin(const QString& itemId);

  /// Unpin (item stays cached, just becomes evictable).
  void unpin();

  /// Stats string for logging.
  QString stats() const;

private Q_SLOTS:
  void pressureCheck();

private:
  struct CacheEntry {
    QString path;
    qint64 size = 0;
    bool complete = false;
    bool downloading = false;
    double lastAccess = 0;
  };

  void evictLru();
  qint64 readMemAvailable() const;
  void doDownload(const QString& itemId, const QString& url);

  QMap<QString, CacheEntry> m_entries;
  QString m_pinnedId;
  QString m_cacheDir;
  QMutex m_mutex;
  QTimer* m_pressureTimer = nullptr;

  static constexpr double PRESSURE_FLOOR_GB = 2.0;
};

#endif // STREAMCACHEMANAGER_H
