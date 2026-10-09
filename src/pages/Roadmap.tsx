import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router";
import { BookOpen, CheckCircle2, Circle, Gauge } from "lucide-react";
import { api, BOOK_STATUSES, errorMessage, type RoadmapStep, type StepChapter } from "../ipc";
import { statusLabel, statusTone, TrackTabs, useGoalTrack } from "./Skills";
import { Badge, Button, Empty, ErrorNote, inputClass, Spinner } from "../ui";

function ChapterLink({ fingerprint, c }: { fingerprint: string; c: StepChapter }) {
  return (
    <Link
      to={`/book/${fingerprint}?page=${c.startPage}`}
      className="inline-flex items-center gap-1.5 text-sm hover:text-accent hover:underline"
    >
      {c.done ? <CheckCircle2 className="size-3.5 shrink-0 text-good" /> : <Circle className="size-3.5 shrink-0 text-muted" />}
      <span>{c.title}</span>
      <span className="text-xs text-muted">
        pp. {c.startPage}–{c.endPage}
      </span>
    </Link>
  );
}

function StepCard({ s, n }: { s: RoadmapStep; n: number }) {
  return (
    <li className="rounded-lg border border-line bg-panel p-4">
      <div className="mb-1 flex flex-wrap items-center gap-2">
        <span className="text-sm font-semibold text-muted">{n}.</span>
        <h3 className="text-sm font-semibold">{s.label}</h3>
        <Badge tone={statusTone[s.status]}>
          {statusLabel[s.status]}
          {s.mastery != null ? ` · ${s.mastery}%` : ""}
        </Badge>
      </div>
      <p className="mb-2 text-xs text-muted">{s.reason}</p>
      {s.book ? (
        <>
          <div className="mb-1 flex items-center gap-1.5 text-sm">
            <BookOpen className="size-3.5 text-accent" />
            <Link to={`/book/${s.book.fingerprint}`} className="font-medium hover:underline">
              {s.book.title}
            </Link>
            <span className="text-xs text-muted">{BOOK_STATUSES.find((x) => x.id === s.book!.status)?.label}</span>
          </div>
          {s.chapters.length > 0 ? (
            <ul className="flex flex-col gap-1 pl-5">
              {s.chapters.map((c) => (
                <li key={c.id}>
                  <ChapterLink fingerprint={s.book!.fingerprint} c={c} />
                </li>
              ))}
            </ul>
          ) : (
            <p className="pl-5 text-xs text-muted">No matching chapters found yet — open the book to browse it.</p>
          )}
          {s.alternatives.length > 0 && (
            <p className="mt-2 text-xs text-muted">
              Also covered in:{" "}
              {s.alternatives.map((b, i) => (
                <span key={b.fingerprint}>
                  {i > 0 && ", "}
                  <Link to={`/book/${b.fingerprint}`} className="hover:underline">
                    {b.title}
                  </Link>
                </span>
              ))}
            </p>
          )}
        </>
      ) : (
        <p className="text-xs text-muted">None of your books cover this topic yet.</p>
      )}
    </li>
  );
}

function PrepareBanner({ track, count, onDone }: { track: string; count: number; onDone: () => void }) {
  const [progress, setProgress] = useState<{ done: number; total: number; title: string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function run() {
    setError(null);
    setProgress({ done: 0, total: count, title: "" });
    try {
      const errors = await api.prepareRoadmap(track, setProgress);
      if (errors.length) setError(`Some books couldn't be read: ${errors.join("; ")}`);
      onDone();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setProgress(null);
    }
  }

  return (
    <div className="mb-5 rounded-lg border border-accent/30 bg-accent-soft p-4 text-sm">
      {progress ? (
        <div className="flex items-center gap-3">
          <Spinner />
          <span>
            Reading {progress.title || "books"}… ({progress.done}/{progress.total})
          </span>
          <div className="h-1 flex-1 overflow-hidden rounded bg-panel">
            <div className="h-full bg-accent" style={{ width: `${(progress.done / Math.max(1, progress.total)) * 100}%` }} />
          </div>
        </div>
      ) : (
        <div className="flex flex-wrap items-center gap-3">
          <span>
            {count} book{count === 1 ? " hasn't" : "s haven't"} been read by the app yet, so the roadmap can't point to
            their chapters.
          </span>
          <Button variant="primary" onClick={run}>
            Find chapters
          </Button>
        </div>
      )}
      <div className="mt-2">
        <ErrorNote error={error} />
      </div>
    </div>
  );
}

export default function RoadmapPage() {
  const qc = useQueryClient();
  const { track, setTrack, prefs } = useGoalTrack();
  const roadmap = useQuery({ queryKey: ["roadmap", track], queryFn: () => api.roadmap(track) });
  const skills = useQuery({ queryKey: ["skills", track], queryFn: () => api.skills(track) });
  const labels = new Map(skills.data?.topics.map((t) => [t.id, t.label]));
  const untested = skills.data?.topics.every((t) => t.mastery == null);
  const r = roadmap.data;

  async function setPace(p: number) {
    qc.setQueryData(["prefs"], await api.setPrefs({ weeklyPace: p }));
    qc.invalidateQueries({ queryKey: ["roadmap"] });
  }

  return (
    <div className="h-full overflow-y-auto">
      <header className="sticky top-0 z-10 flex flex-wrap items-center gap-3 border-b border-line bg-bg/95 px-6 py-3 backdrop-blur">
        <h1 className="text-lg font-semibold">Roadmap</h1>
        <TrackTabs track={track} onChange={setTrack} />
        <label className="ml-auto flex items-center gap-2 text-sm text-muted">
          Pace
          <select
            className={`${inputClass} w-auto`}
            value={prefs?.weeklyPace ?? 3}
            onChange={(e) => setPace(Number(e.target.value))}
          >
            {[1, 2, 3, 4, 5, 6, 8, 10].map((n) => (
              <option key={n} value={n}>
                {n} chapter{n === 1 ? "" : "s"} / week
              </option>
            ))}
          </select>
        </label>
      </header>
      <div className="mx-auto max-w-4xl px-6 py-5">
        {roadmap.isLoading && <Spinner />}
        {roadmap.error && <ErrorNote error={errorMessage(roadmap.error)} />}
        {r && (
          <>
            {untested && (
              <div className="mb-5 flex flex-wrap items-center gap-3 rounded-lg border border-line bg-panel p-4 text-sm">
                <Gauge className="size-5 text-accent" />
                <span className="flex-1">
                  This roadmap follows the usual topic order. Take the skill test to skip what you already know.
                </span>
                <Link to="/skills">
                  <Button variant="primary">Take the skill test</Button>
                </Link>
              </div>
            )}
            {r.needsIndex.length > 0 && (
              <PrepareBanner
                track={track}
                count={r.needsIndex.length}
                onDone={() => qc.invalidateQueries({ queryKey: ["roadmap"] })}
              />
            )}

            {r.steps.length === 0 ? (
              <Empty title="Nothing left on this track">
                Every topic is at 75% or more. Pick another track, or keep your skills fresh with chapter quizzes.
              </Empty>
            ) : (
              <>
                <h2 className="mb-3 text-base font-semibold">Weekly plan</h2>
                {r.weeks.length === 0 ? (
                  <p className="mb-6 text-sm text-muted">
                    No chapters to schedule yet{r.needsIndex.length ? " — use “Find chapters” above" : ""}.
                  </p>
                ) : (
                  <div className="mb-8 grid gap-3 md:grid-cols-2">
                    {r.weeks.map((w) => (
                      <section key={w.number} className="rounded-lg border border-line bg-panel p-4">
                        <div className="mb-2 flex items-baseline gap-2">
                          <h3 className="text-sm font-semibold">Week {w.number}</h3>
                          <span className="text-xs text-muted">
                            from {new Date(`${w.starts}T00:00:00`).toLocaleDateString(undefined, { month: "short", day: "numeric" })} ·
                            ~{w.pages} pages
                          </span>
                        </div>
                        <ul className="flex flex-col gap-2">
                          {w.items.map((it) => (
                            <li key={`${it.book.fingerprint}-${it.chapter.id}`}>
                              <ChapterLink fingerprint={it.book.fingerprint} c={it.chapter} />
                              <div className="pl-5 text-[11px] text-muted">
                                {it.book.title} · {it.topicLabel}
                              </div>
                            </li>
                          ))}
                        </ul>
                      </section>
                    ))}
                  </div>
                )}

                <h2 className="mb-1 text-base font-semibold">Steps</h2>
                <p className="mb-3 text-xs text-muted">
                  Weakest topics first, never before their prerequisites. Updates after every quiz.
                </p>
                <ol className="flex flex-col gap-3">
                  {r.steps.map((s, i) => (
                    <StepCard key={s.topicId} s={s} n={i + 1} />
                  ))}
                </ol>
              </>
            )}

            {r.learned.length > 0 && (
              <div className="mt-6 text-sm">
                <span className="font-medium">Already solid: </span>
                <span className="text-muted">{r.learned.map((id) => labels.get(id) ?? id).join(", ")}</span>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
