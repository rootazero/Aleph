---
name: media-tools
description: Media processing — video, audio, image manipulation via ffmpeg
emoji: "🎬"
category: automation
cli-wrapper: true
requirements:
  binaries:
    - ffmpeg
  install:
    - manager: brew
      package: ffmpeg
triggers:
  - ffmpeg
  - video
  - audio
  - convert video
  - compress
  - GIF
  - thumbnail
---

# Media Processing (ffmpeg)

## When to Use

Invoke this skill for any video, audio, or image processing task: format conversion, compression, clipping, merging, GIF creation, thumbnail extraction, or metadata inspection.

## Prerequisites

```bash
brew install ffmpeg
ffmpeg -version
```

## Core Operations

### Inspect Media Info

```bash
# Show all streams and metadata
ffprobe -v quiet -print_format json -show_format -show_streams input.mp4 | jq '{
  duration: .format.duration,
  size: .format.size,
  video: .streams[] | select(.codec_type=="video") | {codec: .codec_name, width: .width, height: .height, fps: .r_frame_rate},
  audio: .streams[] | select(.codec_type=="audio") | {codec: .codec_name, sample_rate: .sample_rate, channels: .channels}
}'
```

### Video Operations

```bash
# Cut a clip (fast, no re-encoding — -ss BEFORE -i for speed)
ffmpeg -ss 00:01:30 -i input.mp4 -t 00:00:30 -c copy clip.mp4

# Compress video (CRF: 18=high quality, 23=default, 28=small file)
ffmpeg -i input.mp4 -c:v libx264 -crf 23 -preset medium -c:a aac -b:a 128k output.mp4

# Change resolution
ffmpeg -i input.mp4 -vf "scale=1280:720" -c:a copy output_720p.mp4

# Change speed (2x faster)
ffmpeg -i input.mp4 -filter:v "setpts=0.5*PTS" -filter:a "atempo=2.0" fast.mp4

# Merge videos (same codec)
echo "file 'part1.mp4'" > list.txt
echo "file 'part2.mp4'" >> list.txt
ffmpeg -f concat -safe 0 -i list.txt -c copy merged.mp4

# Add watermark
ffmpeg -i input.mp4 -i logo.png -filter_complex "overlay=W-w-10:H-h-10" watermarked.mp4

# Add subtitles
ffmpeg -i input.mp4 -vf "subtitles=subs.srt" -c:a copy subtitled.mp4

# Remove audio
ffmpeg -i input.mp4 -an -c:v copy silent.mp4
```

### Audio Operations

```bash
# Extract audio from video
ffmpeg -i video.mp4 -vn -c:a copy audio.aac
ffmpeg -i video.mp4 -vn -c:a libmp3lame -q:a 2 audio.mp3

# Convert audio format
ffmpeg -i input.wav -c:a libmp3lame -q:a 2 output.mp3
ffmpeg -i input.mp3 -c:a aac -b:a 192k output.m4a

# Adjust volume
ffmpeg -i input.mp3 -filter:a "volume=1.5" louder.mp3
ffmpeg -i input.mp3 -filter:a "volume=0.5" quieter.mp3

# Trim audio
ffmpeg -ss 00:00:10 -i input.mp3 -t 00:00:30 -c copy clip.mp3
```

### Image Operations

```bash
# Extract frame at timestamp
ffmpeg -ss 00:00:05 -i video.mp4 -frames:v 1 frame.jpg

# Generate thumbnail grid
ffmpeg -i video.mp4 -vf "fps=1/10,scale=320:-1,tile=5x4" thumbnails.jpg

# Convert image format
ffmpeg -i input.png output.jpg
ffmpeg -i input.jpg -q:v 2 output.webp

# Resize image
ffmpeg -i input.png -vf "scale=800:-1" resized.png

# Create GIF from video
ffmpeg -ss 00:00:05 -i video.mp4 -t 5 -vf "fps=15,scale=480:-1:flags=lanczos" -loop 0 output.gif
```

## Quality Control

| Parameter | Meaning | Range |
|-----------|---------|-------|
| `-crf` | Constant Rate Factor (H.264) | 0=lossless, 18=high, 23=default, 28=low |
| `-preset` | Encoding speed vs compression | ultrafast, fast, medium, slow, veryslow |
| `-q:a` | MP3 quality (LAME) | 0=best, 2=good, 5=ok, 9=worst |
| `-b:a` | Audio bitrate | 128k=standard, 192k=good, 320k=high |
| `-b:v` | Video bitrate | 1M, 2.5M, 5M (use CRF instead when possible) |

## Critical Gotchas

- **`-ss` position matters**: Before `-i` = fast seek (keyframe), after `-i` = precise but slow
- **`-c copy` + `-vf` incompatible**: Filters require re-encoding. Drop `-c copy` when using `-vf`
- **Overwrite**: Add `-y` to auto-overwrite, or ffmpeg will prompt and block
- **Container vs codec**: `.mp4` is container, `h264` is codec. Wrong combination = errors
- **Audio filters need `-filter:a`**: Not `-vf` (video filter)

## Quick Reference

| Task | Command Pattern |
|------|----------------|
| Get info | `ffprobe -v quiet -print_format json -show_streams FILE` |
| Cut clip | `ffmpeg -ss START -i FILE -t DURATION -c copy OUT` |
| Compress | `ffmpeg -i FILE -c:v libx264 -crf 23 OUT` |
| Extract audio | `ffmpeg -i FILE -vn -c:a copy OUT` |
| Extract frame | `ffmpeg -ss TIME -i FILE -frames:v 1 OUT.jpg` |
| Make GIF | `ffmpeg -ss START -i FILE -t DUR -vf "fps=15,scale=480:-1" OUT.gif` |
| Convert format | `ffmpeg -i FILE OUT.newext` |

## Aleph Integration

- Synergy with `data-pipeline` (A5): ffprobe JSON output → jq processing
- Synergy with `notification` (A7): "Video processing complete" → notify user
