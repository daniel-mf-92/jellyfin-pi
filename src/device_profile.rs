use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DeviceProfile {
    pub max_streaming_bitrate: i64,
    pub max_static_bitrate: i64,
    pub music_streaming_transcoding_bitrate: i64,
    pub direct_play_profiles: Vec<DirectPlayProfile>,
    pub transcoding_profiles: Vec<TranscodingProfile>,
    pub codec_profiles: Vec<CodecProfile>,
    pub subtitle_profiles: Vec<SubtitleProfile>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DirectPlayProfile {
    pub container: String,
    #[serde(rename = "Type")]
    pub profile_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_codec: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct TranscodingProfile {
    pub container: String,
    #[serde(rename = "Type")]
    pub profile_type: String,
    pub video_codec: String,
    pub audio_codec: String,
    pub protocol: String,
    pub context: String,
    pub max_audio_channels: String,
    pub min_segments: i32,
    pub break_on_non_key_frames: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CodecProfile {
    #[serde(rename = "Type")]
    pub profile_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    pub conditions: Vec<ProfileCondition>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ProfileCondition {
    pub condition: String,
    pub property: String,
    pub value: String,
    pub is_required: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SubtitleProfile {
    pub format: String,
    pub method: String,
}

impl DeviceProfile {
    pub fn pi5_vlc() -> Self {
        Self {
            max_streaming_bitrate: 120_000_000,
            max_static_bitrate: 100_000_000,
            music_streaming_transcoding_bitrate: 384_000,
            direct_play_profiles: vec![
                DirectPlayProfile {
                    container: "mp4,m4v,mkv,webm,mov,avi,ts,mpegts,mpg,mpeg,wmv,flv".into(),
                    profile_type: "Video".into(),
                    video_codec: Some("h264,hevc,mpeg2video,mpeg4,vp8,vp9,av1".into()),
                    audio_codec: Some("aac,ac3,eac3,mp3,mp2,opus,vorbis,flac,alac,pcm_s16le,pcm_s24le,dts,truehd".into()),
                },
                DirectPlayProfile {
                    container: "mp3,flac,ogg,oga,opus,webma,m4a,m4b,aac,wav,wma,aiff".into(),
                    profile_type: "Audio".into(),
                    video_codec: None,
                    audio_codec: None,
                },
            ],
            transcoding_profiles: vec![
                TranscodingProfile {
                    container: "ts".into(),
                    profile_type: "Video".into(),
                    video_codec: "h264".into(),
                    audio_codec: "aac,ac3,mp3".into(),
                    protocol: "hls".into(),
                    context: "Streaming".into(),
                    max_audio_channels: "6".into(),
                    min_segments: 1,
                    break_on_non_key_frames: true,
                },
                TranscodingProfile {
                    container: "mp3".into(),
                    profile_type: "Audio".into(),
                    video_codec: String::new(),
                    audio_codec: "mp3".into(),
                    protocol: "http".into(),
                    context: "Streaming".into(),
                    max_audio_channels: "2".into(),
                    min_segments: 0,
                    break_on_non_key_frames: false,
                },
            ],
            codec_profiles: vec![
                CodecProfile {
                    profile_type: "Video".into(),
                    codec: Some("h264".into()),
                    conditions: vec![
                        ProfileCondition { condition: "NotEquals".into(), property: "IsAnamorphic".into(), value: "true".into(), is_required: false },
                        ProfileCondition { condition: "LessThanEqual".into(), property: "VideoLevel".into(), value: "52".into(), is_required: false },
                    ],
                },
                CodecProfile {
                    profile_type: "Video".into(),
                    codec: Some("hevc".into()),
                    conditions: vec![
                        ProfileCondition { condition: "LessThanEqual".into(), property: "VideoLevel".into(), value: "183".into(), is_required: false },
                    ],
                },
                CodecProfile {
                    profile_type: "Video".into(),
                    codec: None,
                    conditions: vec![
                        ProfileCondition { condition: "LessThanEqual".into(), property: "Width".into(), value: "3840".into(), is_required: false },
                        ProfileCondition { condition: "LessThanEqual".into(), property: "Height".into(), value: "2160".into(), is_required: false },
                    ],
                },
            ],
            subtitle_profiles: vec![
                SubtitleProfile { format: "srt".into(), method: "External".into() },
                SubtitleProfile { format: "ass".into(), method: "External".into() },
                SubtitleProfile { format: "ssa".into(), method: "External".into() },
                SubtitleProfile { format: "vtt".into(), method: "External".into() },
                SubtitleProfile { format: "sub".into(), method: "External".into() },
                SubtitleProfile { format: "subrip".into(), method: "Embed".into() },
                SubtitleProfile { format: "pgs".into(), method: "Embed".into() },
                SubtitleProfile { format: "pgssub".into(), method: "Embed".into() },
                SubtitleProfile { format: "dvdsub".into(), method: "Embed".into() },
                SubtitleProfile { format: "dvbsub".into(), method: "Embed".into() },
            ],
        }
    }
}
