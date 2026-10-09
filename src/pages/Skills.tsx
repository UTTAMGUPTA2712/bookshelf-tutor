import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "react-router";
import { ArrowRight, Gauge } from "lucide-react";
import { api, errorMessage, type TopicStatus, type TopicView } from "../ipc";
import { Md, List } from "./Quiz";
import { Badge, Button, ErrorNote, formatDate, scoreTone, Spinner } from "../ui";

export const statusTone: Record<TopicStatus, "good" | "warn" | "bad" | "neutral"> = {
  strong: "good",
  learning: "warn",
  weak: "bad",
  untested: "neutral",
};

export const statusLabel: Record<TopicStatus, string> = {
  strong: "strong",
  learning: "learning",
  weak: "weak",
  untested: "not tested",
};

/** The learner's goal track (synced preference), defaulting to Fundamentals. */
export function useGoalTrack() {
  const qc = useQueryClient();
  const prefs = useQuery({ queryKey: ["prefs"], queryFn: api.prefs });
  const tracks = useQuery({ queryKey: ["skillTracks"], queryFn: api.skillTracks, staleTime: Infinity });
  const track = prefs.data?.goalTrack ?? "fundamentals";
  async function setTrack(id: string) {
    qc.setQueryData(["prefs"], await api.setPrefs({ goalTrack: id }));
  }
  return { track, setTrack, tracks: tracks.data ?? [], prefs: prefs.data, ready: prefs.isSuccess };
}

export function TrackTabs({ track, onChange }: { track: string; onChange: (id: string) => void }) {
  const { tracks } = useGoalTrack();
  return (
    <div className="flex flex-wrap gap-1 rounded-lg bg-sunken p-1">
      {tracks.map((t) => (
        <button
          key={t.id}
          onClick={() => onChange(t.id)}
          className={`rounded-md px-3 py-1 text-sm ${t.id === track ? "bg-panel font-medium text-ink shadow-sm" : "text-muted hover:text-ink"}`}
        >
          {t.label}
        </button>
      ))}
    </div>
  );
}

const barColor: Record<TopicStatus, string> = {
  strong: "bg-good",
  learning: "bg-warn",
  weak: "bg-bad",
  untested: "bg-line",
};

function TopicCard({ t, labels }: { t: TopicView; labels: Map<string, string> }) {
  return (
    <div className="rounded-lg border border-line bg-panel p-3" title={t.description}>
      <div className="mb-1.5 flex items-start justify-between gap-2">
        <h4 className="text-sm leading-snug font-medium">{t.label}</h4>
        <span className="shrink-0 text-sm font-semibold tabular-nums">{t.mastery != null ? `${t.mastery}%` : "—"}</span>
      </div>
      <div className="mb-2 h-1.5 overflow-hidden rounded bg-sunken">
        <div className={`h-full ${barColor[t.status]}`} style={{ width: `${t.mastery ?? 0}%` }} />
      </div>
      <div className="flex flex-wrap items-center gap-1.5 text-[11px] text-muted">
        <Badge tone={statusTone[t.status]}>{statusLabel[t.status]}</Badge>
        {t.evidence > 0 && (
          <span title="How sure the app is about this number — grows with every quiz">
            {t.evidence} result{t.evidence === 1 ? "" : "s"} · {Math.round(t.confidence * 100)}% sure
          </span>
        )}
      </div>
      {t.prereqs.length > 0 && (
        <p className="mt-1.5 text-[11px] text-muted">After: {t.prereqs.map((p) => labels.get(p) ?? p).join(", ")}</p>
      )}
    </div>
  );
}

function StartTest({ track, topicCount, resumeId }: { track: string; topicCount: number; resumeId: string | null }) {
  const navigate = useNavigate();
  const [freeText, setFreeText] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function start() {
    setBusy(true);
    setError(null);
    try {
      const a = await api.startAssessment(track, freeText);
      navigate(`/assessment/${a.id}`);
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  }

  return (
    <div>
      <div className="flex flex-wrap items-center gap-3">
        {resumeId && (
          <Button variant="primary" onClick={() => navigate(`/assessment/${resumeId}`)}>
            Continue skill test
          </Button>
        )}
        <Button variant={resumeId ? "secondary" : "primary"} busy={busy} onClick={start}>
          <Gauge className="size-4" /> {resumeId ? "Start over" : "Take the skill test"}
        </Button>
        <label className="flex items-center gap-1.5 text-sm text-muted">
          <input type="checkbox" checked={freeText} onChange={(e) => setFreeText(e.target.checked)} />
          Include written answers (graded by AI, slower)
        </label>
      </div>
      <p className="mt-2 text-xs text-muted">
        About {topicCount * 2} questions over {topicCount} topics, roughly 15–20 minutes. Questions get harder after a
        right answer and easier after a wrong one; topics that build on something you missed are skipped.
      </p>
      <div className="mt-2">
        <ErrorNote error={error} />
      </div>
    </div>
  );
}

export default function SkillsPage() {
  const { track, setTrack, tracks } = useGoalTrack();
  const map = useQuery({ queryKey: ["skills", track], queryFn: () => api.skills(track) });
  const last = map.data?.lastAssessment;
  const labels = new Map(map.data?.topics.map((t) => [t.id, t.label]));
  const layers: TopicView[][] = [];
  for (const t of map.data?.topics ?? []) (layers[t.layer] ??= []).push(t);
  const tested = map.data?.topics.filter((t) => t.mastery != null).length ?? 0;

  return (
    <div className="h-full overflow-y-auto">
      <header className="sticky top-0 z-10 flex flex-wrap items-center gap-3 border-b border-line bg-bg/95 px-6 py-3 backdrop-blur">
        <h1 className="text-lg font-semibold">Skills</h1>
        <TrackTabs track={track} onChange={setTrack} />
      </header>
      <div className="px-6 py-5">
        {map.isLoading && <Spinner />}
        {map.error && <ErrorNote error={errorMessage(map.error)} />}
        {map.data && (
          <>
            <section className="mb-6 rounded-lg border border-line bg-panel p-5">
              {last?.status === "completed" ? (
                <>
                  <div className="mb-2 flex flex-wrap items-center gap-2">
                    <h2 className="text-base font-semibold">Last skill test</h2>
                    {last.score != null && <Badge tone={scoreTone(last.score)}>{last.score}</Badge>}
                    <span className="text-xs text-muted">{formatDate(last.createdAt)}</span>
                    <Link to={`/assessment/${last.id}`} className="text-xs text-accent hover:underline">
                      See answers
                    </Link>
                  </div>
                  {last.summary && <Md>{last.summary}</Md>}
                  <List title="Strengths" items={last.strengths} tone="text-good" />
                  <List title="Gaps" items={last.focusNext} tone="text-warn" />
                  <div className="mt-4 border-t border-line pt-4">
                    <StartTest track={track} topicCount={map.data.topics.length} resumeId={null} />
                  </div>
                </>
              ) : (
                <>
                  <h2 className="mb-1 text-base font-semibold">Find out where you stand in {map.data.label}</h2>
                  <p className="mb-3 text-sm text-muted">
                    The skill test fills in the map below. Chapter quizzes keep updating it afterwards.
                  </p>
                  <StartTest
                    track={track}
                    topicCount={map.data.topics.length}
                    resumeId={last?.status === "in_progress" ? last.id : null}
                  />
                </>
              )}
            </section>

            <div className="mb-3 flex items-center gap-3">
              <h2 className="text-base font-semibold">Skill map</h2>
              <span className="text-xs text-muted">
                {tested} of {map.data.topics.length} topics measured · columns follow prerequisites, left to right
              </span>
              <Link to="/roadmap" className="ml-auto flex items-center gap-1 text-sm text-accent hover:underline">
                Your roadmap <ArrowRight className="size-3.5" />
              </Link>
            </div>
            <div className="flex gap-3 overflow-x-auto pb-4">
              {layers.map((col, i) => (
                <div key={i} className="flex w-56 shrink-0 flex-col gap-3">
                  {col.map((t) => (
                    <TopicCard key={t.id} t={t} labels={labels} />
                  ))}
                </div>
              ))}
            </div>
            {tracks.length > 0 && (
              <p className="mt-2 text-xs text-muted">
                Mastery mixes skill-test answers (harder questions count for more) with chapter-quiz scores; newer
                results weigh more.
              </p>
            )}
          </>
        )}
      </div>
    </div>
  );
}
