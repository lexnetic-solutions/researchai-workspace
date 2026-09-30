import { useStore } from '../state/store';
import { EmptyState } from '../components/EmptyState';

export function AudioView() {
  const { setView } = useStore();

  return (
    <section className="view">
      <header className="view-head">
        <h1>Audio</h1>
        <p className="view-sub">Lectures in, audio summaries out — all offline.</p>
      </header>

      <div className="card notice">
        <h2>Audio pipeline arrives in Phases 7–8</h2>
        <p>
          Input: FFmpeg normalisation → whisper.cpp transcription → searchable,
          timestamped transcripts. Output: read-aloud, audio summaries and research
          podcast narration via a replaceable TTS provider.
        </p>
        <button type="button" className="btn ghost" onClick={() => setView('library')}>
          Meanwhile: import documents
        </button>
      </div>

      <div className="two-col">
        <div className="card">
          <h2>Planned input</h2>
          <ul className="check-list">
            <li>WAV / MP3 / M4A recordings</li>
            <li>MP4 / MOV lecture videos</li>
            <li>Timestamped transcript viewer</li>
          </ul>
        </div>
        <div className="card">
          <h2>Planned output</h2>
          <ul className="check-list">
            <li>Read aloud (document or passage)</li>
            <li>5 / 10 / 20-minute audio summaries</li>
            <li>Research podcast narration</li>
            <li>MP3 / WAV export</li>
          </ul>
        </div>
      </div>

      <EmptyState title="Nothing to play yet" hint="Audio tools unlock after the document pipeline is live." />
    </section>
  );
}
