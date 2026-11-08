//! Bounded, synchronous binary IPC for the isolated helper process.
//!
//! Frame prefix: 4-byte big-endian payload length (prefix excluded), then one-byte tag.
//! Integers are big-endian. Audio samples are little-endian IEEE-754 f32. No JSON, paths,
//! text logs, or audio/embedding values are emitted in errors.

use std::io::{Read, Write};

use sha2::{Digest, Sha256};

use crate::filter::{FilterSession, Recipe, MAX_HOST_BATCH_SAMPLES};
use crate::models::LoadedModels;
use crate::profile::{ProfileFile, MAX_PROFILE_BYTES};
use crate::{Error, Result};

pub const MAX_FRAME_BYTES: usize = 1_048_576;

const BEGIN: u8 = 1;
const AUDIO: u8 = 2;
const FINISH: u8 = 3;
const CANCEL: u8 = 4;
const READY: u8 = 0x80;
const OUTPUT: u8 = 0x81;
const FINISHED: u8 = 0x82;
const FAILURE: u8 = 0x83;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
enum FailureCode {
    MalformedFrame = 1,
    InvalidState = 2,
    IdentityMismatch = 3,
    NonContiguousAudio = 4,
    InvalidAudio = 5,
    InferenceFailure = 6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StreamId {
    session_id: u64,
    audio_epoch: u64,
}

struct ActiveSession {
    id: StreamId,
    profile_digest: [u8; 32],
    recipe: Recipe,
    filter: FilterSession,
    next_sequence: u64,
    input_samples: u64,
    output_samples: u64,
    output_sequence: u64,
}

pub fn serve_stdio(models: &LoadedModels) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    serve(&mut input, &mut output, models)
}

pub fn serve<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    models: &LoadedModels,
) -> Result<()> {
    let mut active: Option<ActiveSession> = None;
    loop {
        let frame = match read_frame(reader) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                if active.is_some() {
                    write_failure(
                        writer,
                        active.as_ref().map(|s| s.id),
                        FailureCode::InvalidState,
                    )?;
                    return Err(Error::Protocol("input ended before Finish or Cancel"));
                }
                writer
                    .flush()
                    .map_err(|_| Error::Protocol("response channel failed"))?;
                return Ok(());
            }
            Err(_) => {
                write_failure(
                    writer,
                    active.as_ref().map(|s| s.id),
                    FailureCode::MalformedFrame,
                )?;
                return Err(Error::Protocol(
                    "binary input frame is truncated or invalid",
                ));
            }
        };
        if frame.is_empty() {
            write_failure(
                writer,
                active.as_ref().map(|s| s.id),
                FailureCode::MalformedFrame,
            )?;
            return Err(Error::Protocol("binary input frame is invalid"));
        }
        match frame[0] {
            BEGIN => {
                if active.is_some() {
                    let incoming_id = (frame.len() >= 17).then(|| StreamId {
                        session_id: get_u64(&frame, 1).unwrap_or(0),
                        audio_epoch: get_u64(&frame, 9).unwrap_or(0),
                    });
                    write_failure(writer, incoming_id, FailureCode::InvalidState)?;
                    continue;
                }
                match begin_session(&frame, models) {
                    Ok(session) => {
                        write_ready(writer, &session, models)?;
                        active = Some(session);
                    }
                    Err((id, code)) => {
                        write_failure(writer, id, code)?;
                    }
                }
            }
            AUDIO => {
                let audio = match decode_audio(&frame) {
                    Ok(audio) => audio,
                    Err(code) => {
                        write_failure(writer, active.as_ref().map(|s| s.id), code)?;
                        return Err(Error::Protocol("audio message is invalid"));
                    }
                };
                let Some(session) = active.as_mut() else {
                    write_failure(writer, Some(audio.id), FailureCode::InvalidState)?;
                    continue;
                };
                if !same_stream(session.id, audio.id) {
                    // A stale (or foreign) epoch is rejected without touching the active stream.
                    write_failure(writer, Some(audio.id), FailureCode::IdentityMismatch)?;
                    continue;
                }
                if audio.sequence != session.next_sequence
                    || audio.source_start != session.input_samples
                {
                    write_failure(writer, Some(audio.id), FailureCode::NonContiguousAudio)?;
                    return Err(Error::Protocol(
                        "audio sequence or source offset is not contiguous",
                    ));
                }
                let filtered = match session.filter.push(&audio.samples) {
                    Ok(filtered) => filtered,
                    Err(_) => {
                        write_failure(writer, Some(audio.id), FailureCode::InvalidAudio)?;
                        return Err(Error::Protocol("audio processing failed"));
                    }
                };
                session.next_sequence += 1;
                session.input_samples += audio.samples.len() as u64;
                emit_output(writer, session, &filtered)?;
            }
            FINISH => {
                let finish = match decode_finish(&frame) {
                    Ok(finish) => finish,
                    Err(code) => {
                        write_failure(writer, active.as_ref().map(|s| s.id), code)?;
                        return Err(Error::Protocol("finish message is invalid"));
                    }
                };
                let Some(session) = active.as_mut() else {
                    write_failure(writer, Some(finish.id), FailureCode::InvalidState)?;
                    continue;
                };
                if !same_stream(session.id, finish.id) {
                    write_failure(writer, Some(finish.id), FailureCode::IdentityMismatch)?;
                    continue;
                }
                if finish.input_samples != session.input_samples
                    || finish.next_sequence != session.next_sequence
                {
                    write_failure(writer, Some(finish.id), FailureCode::NonContiguousAudio)?;
                    return Err(Error::Protocol("finish counts do not match received audio"));
                }
                let filtered = match session.filter.finish() {
                    Ok(filtered) => filtered,
                    Err(_) => {
                        write_failure(writer, Some(finish.id), FailureCode::InferenceFailure)?;
                        return Err(Error::Protocol("final filter flush failed"));
                    }
                };
                emit_output(writer, session, &filtered)?;
                if session.input_samples != session.output_samples {
                    write_failure(writer, Some(finish.id), FailureCode::InferenceFailure)?;
                    return Err(Error::Protocol(
                        "filtered output length did not match input",
                    ));
                }
                write_finished(writer, session, 0)?;
                active = None;
            }
            CANCEL => {
                let id = match decode_cancel(&frame) {
                    Ok(id) => id,
                    Err(code) => {
                        write_failure(writer, active.as_ref().map(|s| s.id), code)?;
                        return Err(Error::Protocol("cancel message is invalid"));
                    }
                };
                let Some(session) = active.as_mut() else {
                    write_failure(writer, Some(id), FailureCode::InvalidState)?;
                    continue;
                };
                if !same_stream(session.id, id) {
                    write_failure(writer, Some(id), FailureCode::IdentityMismatch)?;
                    continue;
                }
                write_finished(writer, session, 1)?;
                active = None;
            }
            _ => {
                write_failure(
                    writer,
                    active.as_ref().map(|s| s.id),
                    FailureCode::MalformedFrame,
                )?;
                return Err(Error::Protocol("binary message type is invalid"));
            }
        }
        writer
            .flush()
            .map_err(|_| Error::Protocol("response channel failed"))?;
    }
}

fn begin_session(
    frame: &[u8],
    models: &LoadedModels,
) -> std::result::Result<ActiveSession, (Option<StreamId>, FailureCode)> {
    const FIXED: usize = 1 + 8 + 8 + 1 + 32 + 32 + 4;
    if frame.len() < FIXED {
        return Err((None, FailureCode::MalformedFrame));
    }
    let id = StreamId {
        session_id: get_u64(frame, 1).ok_or((None, FailureCode::MalformedFrame))?,
        audio_epoch: get_u64(frame, 9).ok_or((None, FailureCode::MalformedFrame))?,
    };
    if id.session_id == 0 || id.audio_epoch == 0 {
        return Err((Some(id), FailureCode::IdentityMismatch));
    }
    let recipe = match frame[17] {
        0 => Recipe::G0,
        1 => Recipe::G1,
        _ => return Err((Some(id), FailureCode::MalformedFrame)),
    };
    let model_digest = &frame[18..50];
    let expected_model_digest = hex::decode(&models.digest_sha256)
        .map_err(|_| (Some(id), FailureCode::IdentityMismatch))?;
    if model_digest != expected_model_digest {
        return Err((Some(id), FailureCode::IdentityMismatch));
    }
    let profile_digest = &frame[50..82];
    let profile_len = get_u32(frame, 82).ok_or((Some(id), FailureCode::MalformedFrame))? as usize;
    if profile_len == 0 || profile_len > MAX_PROFILE_BYTES || frame.len() != FIXED + profile_len {
        return Err((Some(id), FailureCode::MalformedFrame));
    }
    let profile_bytes = &frame[FIXED..];
    let actual_profile_digest = Sha256::digest(profile_bytes);
    if profile_digest != actual_profile_digest.as_slice() {
        return Err((Some(id), FailureCode::IdentityMismatch));
    }
    let profile_digest: [u8; 32] = actual_profile_digest.into();
    let (profile, _) = ProfileFile::decode(profile_bytes, models)
        .map_err(|_| (Some(id), FailureCode::IdentityMismatch))?;
    let filter = FilterSession::new(models, &profile, recipe)
        .map_err(|_| (Some(id), FailureCode::InferenceFailure))?;
    Ok(ActiveSession {
        id,
        profile_digest,
        recipe,
        filter,
        next_sequence: 0,
        input_samples: 0,
        output_samples: 0,
        output_sequence: 0,
    })
}

#[derive(Debug, PartialEq)]
struct AudioMessage {
    id: StreamId,
    sequence: u64,
    source_start: u64,
    samples: Vec<f32>,
}

fn same_stream(current: StreamId, incoming: StreamId) -> bool {
    current == incoming
}

fn decode_audio(frame: &[u8]) -> std::result::Result<AudioMessage, FailureCode> {
    const HEADER: usize = 1 + 8 * 4 + 2;
    if frame.len() < HEADER {
        return Err(FailureCode::MalformedFrame);
    }
    let id = StreamId {
        session_id: get_u64(frame, 1).ok_or(FailureCode::MalformedFrame)?,
        audio_epoch: get_u64(frame, 9).ok_or(FailureCode::MalformedFrame)?,
    };
    let sequence = get_u64(frame, 17).ok_or(FailureCode::MalformedFrame)?;
    let source_start = get_u64(frame, 25).ok_or(FailureCode::MalformedFrame)?;
    let valid_samples = get_u16(frame, 33).ok_or(FailureCode::MalformedFrame)? as usize;
    if id.session_id == 0 || id.audio_epoch == 0 {
        return Err(FailureCode::IdentityMismatch);
    }
    if valid_samples == 0
        || valid_samples > MAX_HOST_BATCH_SAMPLES
        || frame.len() != HEADER + valid_samples * 4
    {
        return Err(FailureCode::MalformedFrame);
    }
    let mut samples = Vec::with_capacity(valid_samples);
    for chunk in frame[HEADER..].as_chunks::<4>().0 {
        let sample = f32::from_le_bytes(*chunk);
        if !sample.is_finite() {
            return Err(FailureCode::InvalidAudio);
        }
        samples.push(sample);
    }
    Ok(AudioMessage {
        id,
        sequence,
        source_start,
        samples,
    })
}

struct FinishMessage {
    id: StreamId,
    input_samples: u64,
    next_sequence: u64,
}

fn decode_finish(frame: &[u8]) -> std::result::Result<FinishMessage, FailureCode> {
    if frame.len() != 1 + 8 * 4 {
        return Err(FailureCode::MalformedFrame);
    }
    let id = StreamId {
        session_id: get_u64(frame, 1).ok_or(FailureCode::MalformedFrame)?,
        audio_epoch: get_u64(frame, 9).ok_or(FailureCode::MalformedFrame)?,
    };
    if id.session_id == 0 || id.audio_epoch == 0 {
        return Err(FailureCode::IdentityMismatch);
    }
    Ok(FinishMessage {
        id,
        input_samples: get_u64(frame, 17).ok_or(FailureCode::MalformedFrame)?,
        next_sequence: get_u64(frame, 25).ok_or(FailureCode::MalformedFrame)?,
    })
}

fn decode_cancel(frame: &[u8]) -> std::result::Result<StreamId, FailureCode> {
    if frame.len() != 1 + 8 * 2 {
        return Err(FailureCode::MalformedFrame);
    }
    let id = StreamId {
        session_id: get_u64(frame, 1).ok_or(FailureCode::MalformedFrame)?,
        audio_epoch: get_u64(frame, 9).ok_or(FailureCode::MalformedFrame)?,
    };
    if id.session_id == 0 || id.audio_epoch == 0 {
        return Err(FailureCode::IdentityMismatch);
    }
    Ok(id)
}

fn emit_output<W: Write>(
    writer: &mut W,
    session: &mut ActiveSession,
    samples: &[f32],
) -> Result<()> {
    for chunk in samples.chunks(MAX_HOST_BATCH_SAMPLES) {
        if chunk.is_empty() || chunk.iter().any(|sample| !sample.is_finite()) {
            return Err(Error::Protocol("filtered audio output is invalid"));
        }
        let source_start = session.output_samples;
        let mut frame = Vec::with_capacity(1 + 8 * 4 + 2 + chunk.len() * 4);
        frame.push(OUTPUT);
        frame.extend_from_slice(&session.id.session_id.to_be_bytes());
        frame.extend_from_slice(&session.id.audio_epoch.to_be_bytes());
        frame.extend_from_slice(&session.output_sequence.to_be_bytes());
        frame.extend_from_slice(&source_start.to_be_bytes());
        frame.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
        for &sample in chunk {
            frame.extend_from_slice(&sample.to_le_bytes());
        }
        write_frame(writer, &frame)?;
        session.output_sequence += 1;
        session.output_samples += chunk.len() as u64;
    }
    Ok(())
}

fn write_ready<W: Write>(
    writer: &mut W,
    session: &ActiveSession,
    models: &LoadedModels,
) -> Result<()> {
    let mut frame = Vec::with_capacity(1 + 16 + 65);
    frame.push(READY);
    frame.extend_from_slice(&session.id.session_id.to_be_bytes());
    frame.extend_from_slice(&session.id.audio_epoch.to_be_bytes());
    frame.extend_from_slice(
        &hex::decode(&models.digest_sha256)
            .map_err(|_| Error::Protocol("model digest is invalid"))?,
    );
    frame.extend_from_slice(&session.profile_digest);
    frame.push(match session.recipe {
        Recipe::G0 => 0,
        Recipe::G1 => 1,
    });
    write_frame(writer, &frame)?;
    writer
        .flush()
        .map_err(|_| Error::Protocol("response channel failed"))
}

fn write_finished<W: Write>(writer: &mut W, session: &ActiveSession, status: u8) -> Result<()> {
    let mut frame = Vec::with_capacity(1 + 8 * 4 + 1);
    frame.push(FINISHED);
    frame.extend_from_slice(&session.id.session_id.to_be_bytes());
    frame.extend_from_slice(&session.id.audio_epoch.to_be_bytes());
    frame.push(status);
    frame.extend_from_slice(&session.input_samples.to_be_bytes());
    frame.extend_from_slice(&session.output_samples.to_be_bytes());
    write_frame(writer, &frame)
}

fn write_failure<W: Write>(writer: &mut W, id: Option<StreamId>, code: FailureCode) -> Result<()> {
    let (session_id, audio_epoch) = id.map_or((0, 0), |id| (id.session_id, id.audio_epoch));
    let mut frame = Vec::with_capacity(1 + 8 + 8 + 2);
    frame.push(FAILURE);
    frame.extend_from_slice(&session_id.to_be_bytes());
    frame.extend_from_slice(&audio_epoch.to_be_bytes());
    frame.extend_from_slice(&(code as u16).to_be_bytes());
    write_frame(writer, &frame)?;
    writer
        .flush()
        .map_err(|_| Error::Protocol("response channel failed"))
}

pub fn read_frame<R: Read>(reader: &mut R) -> Result<Option<Vec<u8>>> {
    let mut prefix = [0_u8; 4];
    let first = loop {
        match reader.read(&mut prefix[..1]) {
            Ok(0) => return Ok(None),
            Ok(n) => break n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Error::Protocol("binary frame header could not be read")),
        }
    };
    if first != 1 {
        return Err(Error::Protocol("binary frame header is truncated"));
    }
    reader
        .read_exact(&mut prefix[1..])
        .map_err(|_| Error::Protocol("binary frame header is truncated"))?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(Error::Protocol("binary frame length is invalid"));
    }
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .map_err(|_| Error::Protocol("binary frame body is truncated"))?;
    Ok(Some(payload))
}

pub fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> Result<()> {
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err(Error::Protocol("binary response frame length is invalid"));
    }
    let length = u32::try_from(payload.len())
        .map_err(|_| Error::Protocol("binary response frame length is invalid"))?;
    writer
        .write_all(&length.to_be_bytes())
        .map_err(|_| Error::Protocol("response channel failed"))?;
    writer
        .write_all(payload)
        .map_err(|_| Error::Protocol("response channel failed"))
}

fn get_u64(frame: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        frame.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn get_u32(frame: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        frame.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn get_u16(frame: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        frame.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FragmentedReader {
        bytes: Vec<u8>,
        offset: usize,
        fragment: usize,
    }

    impl Read for FragmentedReader {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.offset == self.bytes.len() {
                return Ok(0);
            }
            let take = out
                .len()
                .min(self.fragment)
                .min(self.bytes.len() - self.offset);
            out[..take].copy_from_slice(&self.bytes[self.offset..self.offset + take]);
            self.offset += take;
            Ok(take)
        }
    }

    fn framed(payload: &[u8]) -> Vec<u8> {
        let mut frame = Vec::new();
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    #[test]
    fn frame_reader_handles_fragmented_prefix_and_body() {
        let expected = vec![AUDIO, 1, 2, 3, 4, 5];
        let mut input = FragmentedReader {
            bytes: framed(&expected),
            offset: 0,
            fragment: 2,
        };
        assert_eq!(read_frame(&mut input).unwrap(), Some(expected));
        assert_eq!(read_frame(&mut input).unwrap(), None);
    }

    #[test]
    fn frame_reader_rejects_truncated_header_body_and_oversized_lengths() {
        let mut header = FragmentedReader {
            bytes: vec![0, 0],
            offset: 0,
            fragment: 1,
        };
        assert!(read_frame(&mut header).is_err());
        let mut body = FragmentedReader {
            bytes: vec![0, 0, 0, 4, 1, 2],
            offset: 0,
            fragment: 2,
        };
        assert!(read_frame(&mut body).is_err());
        let mut large = FragmentedReader {
            bytes: (MAX_FRAME_BYTES as u32 + 1).to_be_bytes().to_vec(),
            offset: 0,
            fragment: 4,
        };
        assert!(read_frame(&mut large).is_err());
    }

    #[test]
    fn audio_decoder_rejects_invalid_ids_lengths_and_non_finite_samples() {
        let mut invalid_id = vec![AUDIO];
        invalid_id.extend_from_slice(&0_u64.to_be_bytes());
        invalid_id.extend_from_slice(&1_u64.to_be_bytes());
        invalid_id.extend_from_slice(&0_u64.to_be_bytes());
        invalid_id.extend_from_slice(&0_u64.to_be_bytes());
        invalid_id.extend_from_slice(&1_u16.to_be_bytes());
        invalid_id.extend_from_slice(&0.0_f32.to_le_bytes());
        assert_eq!(
            decode_audio(&invalid_id),
            Err(FailureCode::IdentityMismatch)
        );

        let mut non_finite = invalid_id.clone();
        non_finite[1..9].copy_from_slice(&1_u64.to_be_bytes());
        non_finite[35..39].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_eq!(decode_audio(&non_finite), Err(FailureCode::InvalidAudio));

        let mut truncated = non_finite;
        truncated.pop();
        assert_eq!(decode_audio(&truncated), Err(FailureCode::MalformedFrame));
    }

    #[test]
    fn stale_epoch_identity_is_not_equal_to_an_active_stream() {
        let active = StreamId {
            session_id: 11,
            audio_epoch: 8,
        };
        let stale = StreamId {
            session_id: 11,
            audio_epoch: 7,
        };
        assert!(!same_stream(active, stale));
        assert!(!same_stream(
            active,
            StreamId {
                session_id: 12,
                audio_epoch: 8
            }
        ));
        assert!(same_stream(active, active));
    }
}
