//! Installed SAPI dictation over a finite, in-memory voice-turn clip.

use std::ffi::c_void;
use std::io::Read;
use std::time::Instant;

use aleph_desktop::media_types::SpeechToTextResult;
use aleph_desktop::{DesktopError, Result};
use aleph_protocol::panel_voice::{MAX_WAV_BYTES, SAMPLE_RATE_HZ, WAV_HEADER_BYTES};
use windows::core::{w, IUnknown, Interface, GUID, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Globalization::{
    LCIDToLocaleName, LocaleNameToLCID, LOCALE_ALLOW_NEUTRAL_NAMES,
};
use windows::Win32::Media::Audio::WAVEFORMATEX;
use windows::Win32::Media::Speech::*;
use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED, STREAM_SEEK_SET,
};
use windows::Win32::System::Threading::WaitForSingleObject;

use super::{
    deadline_exceeded, no_default_token, platform, MAX_LOCALE_UTF16, MAX_TRANSCRIPT_BYTES,
};

const LOCALE_NAME_MAX_LENGTH: usize = MAX_LOCALE_UTF16 + 1;
const LOCALE_CUSTOM_UNSPECIFIED: u16 = 0x1000;
const WAVE_FORMAT: GUID = GUID::from_u128(0xc31adbae_527f_4ff5_a230_f62bb61ff70c);
// Not projected by windows-rs; see the Windows SDK's sperror.h.
const SPERR_NOT_FOUND: HRESULT = HRESULT(0x8004_503a_u32 as i32);

pub(super) fn recognize(
    audio_path: &std::path::Path,
    language: Option<&str>,
    deadline: Instant,
) -> Result<SpeechToTextResult> {
    check_deadline(deadline)?;
    let locale = language.map(requested_langid).transpose()?;
    let mut wav = Vec::new();
    std::fs::File::open(audio_path)
        .and_then(|file| file.take(MAX_WAV_BYTES as u64 + 1).read_to_end(&mut wav))
        .map_err(|e| platform(format!("read offline speech input: {:?}", e.kind())))?;
    recognize_pcm(voice_pcm(&wav)?, language.zip(locale), deadline, |_| {})
}

fn voice_pcm(wav: &[u8]) -> Result<&[u8]> {
    let invalid = || platform("offline speech input is not a bounded 16 kHz mono PCM WAV");
    if wav.len() > MAX_WAV_BYTES {
        return Err(invalid());
    }
    let header = wav.get(..WAV_HEADER_BYTES).ok_or_else(invalid)?;
    let pcm = &wav[WAV_HEADER_BYTES..];
    let read_u32 = |offset| u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
    if &header[..4] != b"RIFF"
        || read_u32(4) as usize != wav.len() - 8
        || &header[8..16] != b"WAVEfmt "
        || read_u32(16) != 16
        || &header[20..24] != b"\x01\0\x01\0"
        || read_u32(24) != SAMPLE_RATE_HZ
        || read_u32(28) != SAMPLE_RATE_HZ * 2
        || &header[32..36] != b"\x02\0\x10\0"
        || &header[36..40] != b"data"
        || read_u32(40) as usize != pcm.len()
        || pcm.is_empty()
        || !pcm.len().is_multiple_of(2)
    {
        return Err(invalid());
    }
    Ok(pcm)
}

fn requested_langid(language: &str) -> Result<u16> {
    let unsupported = || DesktopError::UnsupportedLanguage(language.to_owned());
    // Keep neutral locales neutral, so e.g. `en` never silently becomes `en-US`.
    let mut wide: Vec<_> = language
        .encode_utf16()
        .take(LOCALE_NAME_MAX_LENGTH)
        .collect();
    if wide.is_empty() || wide.len() >= LOCALE_NAME_MAX_LENGTH || wide.contains(&0) {
        return Err(unsupported());
    }
    wide.push(0);
    let lcid = unsafe { LocaleNameToLCID(PCWSTR(wide.as_ptr()), LOCALE_ALLOW_NEUTRAL_NAMES) };
    let langid = lcid as u16;
    // LOCALE_CUSTOM_UNSPECIFIED is shared by distinct newer locale names.
    if langid == 0 || langid == LOCALE_CUSTOM_UNSPECIFIED {
        return Err(unsupported());
    }
    Ok(langid)
}

fn token_langids(metadata: &str) -> impl Iterator<Item = u16> + '_ {
    metadata
        .split(';')
        .filter_map(|hex| u16::from_str_radix(hex.trim(), 16).ok())
}

fn token_supports_langid(metadata: &str, requested: u16) -> bool {
    token_langids(metadata).any(|id| id == requested)
}

fn token_language(token: &ISpObjectToken) -> Result<String> {
    let attributes = native(unsafe { token.OpenKey(w!("Attributes")) })?;
    let language = ComString(native(unsafe {
        attributes.GetStringValue(w!("Language"))
    })?);
    language.copy(1024)
}

fn select_token(
    requested: Option<(&str, u16)>,
    deadline: Instant,
) -> Result<(ISpObjectToken, String)> {
    let category: ISpObjectTokenCategory =
        native(unsafe { CoCreateInstance(&SpObjectTokenCategory, None, CLSCTX_INPROC_SERVER) })?;
    native(unsafe { category.SetId(SPCAT_RECOGNIZERS, false) })?;
    if let Some((language, langid)) = requested {
        let installed = native(unsafe { category.EnumTokens(PCWSTR::null(), PCWSTR::null()) })?;
        let mut count = 0;
        native(unsafe { installed.GetCount(&mut count) })?;
        for index in 0..count {
            check_deadline(deadline)?;
            let token = native(unsafe { installed.Item(index) })?;
            if token_language(&token).is_ok_and(|raw| token_supports_langid(&raw, langid)) {
                return Ok((token, language.to_owned()));
            }
        }
        return Err(DesktopError::UnsupportedLanguage(language.to_owned()));
    }

    let default = match unsafe { category.GetDefaultTokenId() } {
        Ok(id) if !id.is_null() => ComString(id),
        Ok(_) => return Err(no_default_token()),
        Err(e) if e.code() == SPERR_NOT_FOUND => return Err(no_default_token()),
        Err(e) => return native(Err(e)),
    };
    let token: ISpObjectToken =
        native(unsafe { CoCreateInstance(&SpObjectToken, None, CLSCTX_INPROC_SERVER) })?;
    native(unsafe { token.SetId(SPCAT_RECOGNIZERS, PCWSTR(default.0 .0), false) })?;
    let metadata = token_language(&token)?;
    let langid = token_langids(&metadata)
        .next()
        .ok_or_else(|| platform("SAPI default token has no language"))?;
    let mut wide = [0; LOCALE_NAME_MAX_LENGTH];
    let written = unsafe {
        LCIDToLocaleName(
            u32::from(langid),
            Some(&mut wide),
            LOCALE_ALLOW_NEUTRAL_NAMES,
        )
    };
    if written <= 1 {
        return Err(platform("SAPI default token has an unknown language"));
    }
    let language = String::from_utf16(&wide[..written as usize - 1])
        .map_err(|_| platform("invalid SAPI default locale name"))?;
    Ok((token, language))
}

fn recognize_pcm(
    pcm: &[u8],
    requested: Option<(&str, u16)>,
    deadline: Instant,
    mut observe_event: impl FnMut(SPEVENTENUM),
) -> Result<SpeechToTextResult> {
    check_deadline(deadline)?;
    native(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() })?;
    let _apartment = Apartment;
    let (token, language) = select_token(requested, deadline)?;
    check_deadline(deadline)?;

    if pcm.iter().all(|&byte| byte == 0) {
        return Ok(SpeechToTextResult {
            text: String::new(),
            language,
        });
    }

    let base = native(unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) })?;
    let mut written = 0;
    let bytes = pcm.len() as u32;
    native(unsafe {
        base.Write(pcm.as_ptr().cast(), bytes, Some(&mut written))
            .ok()
    })?;
    if written != bytes {
        return Err(platform("short write to SAPI input stream"));
    }
    native(unsafe { base.Seek(0, STREAM_SEEK_SET, None) })?;
    let mut stream = InputStream(Some(native(unsafe {
        CoCreateInstance::<_, ISpStream>(&SpStream, None, CLSCTX_INPROC_SERVER)
    })?));
    let format = WAVEFORMATEX {
        wFormatTag: 1,
        nChannels: 1,
        nSamplesPerSec: SAMPLE_RATE_HZ,
        nAvgBytesPerSec: SAMPLE_RATE_HZ * 2,
        nBlockAlign: 2,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    let input = stream.0.as_ref().unwrap();
    native(unsafe { input.SetBaseStream(&base, &WAVE_FORMAT, &format) })?;

    let recognizer: ISpRecognizer =
        native(unsafe { CoCreateInstance(&SpInprocRecognizer, None, CLSCTX_INPROC_SERVER) })?;
    native(unsafe { recognizer.SetRecognizer(&token) })?;
    let selected = native(unsafe { recognizer.GetRecognizer() })?;
    let actual = ComString(native(unsafe { selected.GetId() })?).copy(2048)?;
    let expected = ComString(native(unsafe { token.GetId() })?).copy(2048)?;
    if !actual.eq_ignore_ascii_case(&expected) {
        return Err(platform("SAPI selected a different recognizer token"));
    }
    drop(selected);
    native(unsafe { recognizer.SetInput(input, true) })?;
    let context = native(unsafe { recognizer.CreateRecoContext() })?;
    native(unsafe { context.SetNotifyWin32Event() })?;
    let notify = unsafe { context.GetNotifyEventHandle() };
    if notify.is_invalid() {
        return Err(platform("SAPI notification event is unavailable"));
    }
    let interest = [
        SPEI_RESERVED1,
        SPEI_RESERVED2,
        SPEI_START_SR_STREAM,
        SPEI_RECOGNITION,
        SPEI_END_SR_STREAM,
    ]
    .into_iter()
    .fold(0, |mask, event| mask | (1u64 << event.0));
    native(unsafe { context.SetInterest(interest, interest) })?;
    let grammar = native(unsafe { context.CreateGrammar(1) })?;
    native(unsafe { grammar.LoadDictation(PCWSTR::null(), SPLO_STATIC) })?;
    check_deadline(deadline)?;

    let result = (|| {
        native(unsafe { grammar.SetDictationState(SPRS_ACTIVE) })?;
        native(unsafe { recognizer.SetRecoState(SPRST_ACTIVE_ALWAYS) })?;
        let mut transcript = Transcript::default();
        let mut stream_number = None;
        loop {
            check_deadline(deadline)?;
            let Some(event) = next_event(&context)? else {
                wait_for_event(notify, deadline)?;
                continue;
            };
            let number = event.0.ulStreamNum;
            match event.id() {
                SPEI_START_SR_STREAM if stream_number.is_none() && event.scalar() => {
                    stream_number = Some(number);
                }
                SPEI_RECOGNITION if stream_number == Some(number) => {
                    let (text, display) = event.text()?;
                    transcript.append(&text, display)?;
                }
                SPEI_END_SR_STREAM if stream_number == Some(number) && event.scalar() => {
                    let text = finish_stream(transcript, HRESULT(event.0.lParam.0 as i32))?;
                    observe_event(SPEI_END_SR_STREAM);
                    break Ok(text);
                }
                _ => return Err(platform("unexpected SAPI stream event")),
            }
            observe_event(event.id());
        }
    })();
    // Purge before releasing the grammar/context and closing the finite input.
    let stopped = native(unsafe { recognizer.SetRecoState(SPRST_INACTIVE_WITH_PURGE) });
    for _ in 0..256 {
        if !matches!(next_event(&context), Ok(Some(_))) {
            break;
        }
    }
    drop(grammar);
    drop(context);
    drop(recognizer);
    let closed = stream.close();
    let text = result?;
    stopped?;
    closed?;
    check_deadline(deadline)?;
    Ok(SpeechToTextResult { text, language })
}

fn wait_for_event(notify: HANDLE, deadline: Instant) -> Result<()> {
    let wait_ms = deadline
        .saturating_duration_since(Instant::now())
        .as_millis()
        .clamp(1, u128::from(u32::MAX - 1)) as u32;
    match unsafe { WaitForSingleObject(notify, wait_ms) } {
        WAIT_OBJECT_0 | WAIT_TIMEOUT => Ok(()),
        _ => Err(platform("wait for SAPI recognition event failed")),
    }
}

#[derive(Default)]
struct Transcript {
    text: String,
    trailing_spaces: usize,
}

impl Transcript {
    fn append(&mut self, text: &str, display: u8) -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        let spaces = if display & SPAF_CONSUME_LEADING_SPACES.0 as u8 != 0 {
            0
        } else {
            self.trailing_spaces
        };
        if self.text.len() + spaces + text.len() > MAX_TRANSCRIPT_BYTES {
            return Err(platform("SAPI transcript exceeded the byte limit"));
        }
        self.text.extend(std::iter::repeat_n(' ', spaces));
        self.text.push_str(text);
        self.trailing_spaces = if display & SPAF_TWO_TRAILING_SPACES.0 as u8 != 0 {
            2
        } else {
            usize::from(display & SPAF_ONE_TRAILING_SPACE.0 as u8 != 0)
        };
        Ok(())
    }
}

fn finish_stream(transcript: Transcript, status: HRESULT) -> Result<String> {
    native(status.ok())?;
    Ok(transcript.text)
}

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct InputStream(Option<ISpStream>);

impl InputStream {
    fn close(&mut self) -> Result<()> {
        if let Some(stream) = self.0.take() {
            native(unsafe { stream.Close() })?;
        }
        Ok(())
    }
}

impl Drop for InputStream {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

struct ComString(PWSTR);

impl ComString {
    fn copy(&self, limit: usize) -> Result<String> {
        if !self.0.is_null() {
            // SAPI's NUL-terminated output is valid for the life of the allocation.
            let raw = self.0 .0;
            for count in 0..=limit {
                if unsafe { *raw.add(count) } == 0 {
                    let decoded =
                        String::from_utf16(unsafe { std::slice::from_raw_parts(raw, count) })
                            .map_err(|_| platform("invalid SAPI UTF-16 string"))?;
                    if decoded.len() <= limit {
                        return Ok(decoded);
                    }
                    break;
                }
            }
        }
        Err(platform("missing or oversized SAPI string"))
    }
}

impl Drop for ComString {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0 .0.cast())) };
    }
}

struct SpeechEvent(SPEVENT);

impl SpeechEvent {
    fn kind(&self) -> SPEVENTLPARAMTYPE {
        SPEVENTLPARAMTYPE(((self.0._bitfield as u32) >> 16) as i32)
    }

    fn scalar(&self) -> bool {
        self.kind() == SPET_LPARAM_IS_UNDEFINED
    }

    fn id(&self) -> SPEVENTENUM {
        SPEVENTENUM(self.0._bitfield & 0xffff)
    }

    fn text(&self) -> Result<(String, u8)> {
        if self.kind() != SPET_LPARAM_IS_OBJECT {
            return Err(platform("unexpected SAPI final event payload"));
        }
        let raw = self.0.lParam.0 as *mut c_void;
        let object = unsafe { IUnknown::from_raw_borrowed(&raw) }
            .ok_or_else(|| platform("missing SAPI final event payload"))?;
        let result: ISpRecoResult = native(object.cast())?;
        let mut text = ComString(PWSTR::null());
        let mut display = 0;
        native(unsafe {
            result.GetText(
                0,
                SPPR_ALL_ELEMENTS.0 as u32,
                true,
                &mut text.0,
                Some(&mut display),
            )
        })?;
        Ok((text.copy(MAX_TRANSCRIPT_BYTES)?, display))
    }
}

impl Drop for SpeechEvent {
    fn drop(&mut self) {
        let raw = self.0.lParam.0 as *mut c_void;
        if !raw.is_null() {
            // Match SDK SpClearEvent. Scalars (including EOS HRESULT) carry no allocation.
            unsafe {
                match self.kind() {
                    SPET_LPARAM_IS_TOKEN | SPET_LPARAM_IS_OBJECT => drop(IUnknown::from_raw(raw)),
                    SPET_LPARAM_IS_POINTER | SPET_LPARAM_IS_STRING => CoTaskMemFree(Some(raw)),
                    _ => {}
                }
            }
        }
    }
}

fn next_event(context: &ISpRecoContext) -> Result<Option<SpeechEvent>> {
    let mut event = SPEVENT::default();
    let mut fetched = 0;
    native(unsafe { context.GetEvents(1, &mut event, &mut fetched) })?;
    Ok((fetched != 0).then(|| SpeechEvent(event)))
}

fn check_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        return Err(deadline_exceeded());
    }
    Ok(())
}

fn native<T>(result: windows::core::Result<T>) -> Result<T> {
    // Keep errors useful without recording paths, locale metadata or speech.
    result.map_err(|e| platform(format!("SAPI HRESULT 0x{:08x}", e.code().0 as u32)))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
