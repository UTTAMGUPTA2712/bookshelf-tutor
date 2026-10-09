import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "react-router";
import { ArrowLeft, BookOpen, Check, CircleDashed, Lightbulb, Minus, X } from "lucide-react";
import { api, errorMessage, newRequestId, type Interview, type PointStatus, type StreamEvent } from "../ipc";
import { Md, List } from "./Quiz";
import { Avatar, verdictLabel, verdictTone } from "./Interviews";
import { StreamBox } from "../components/StreamBox";
import { Badge, Button, ErrorNote, inputClass, scoreTone, Spinner } from "../ui";

/** Pulls the "message" string out of a JSON reply that is still streaming, so the interviewer seems to type. */
function streamedMessage(raw: string): string {
  const m = /"message"\s*:\s*"/.exec(raw);
  if (!m) return "";
  let out = "";
  for (let i = m.index + m[0].length; i < raw.length; i++) {
    const c = raw[i];
    if (c === '"') break;
    if (c === "\\") {
      const n = raw[++i];
      if (n === undefined) break;
      out += n === "n" ? "\n" : n === "t" ? "\t" : n;
    } else out += c;
  }
  return out;
}

/** Main questions per interview length (mirrors `question_count` in interview.rs). */
export const plannedQuestions = (minutes: number) => (minutes <= 20 ? 4 : minutes <= 35 ? 7 : 10);

const difficultyTone = { easy: "good", medium: "accent", hard: "warn" } as const;

const pointIcon: Record<PointStatus, React.ReactNode> = {
  verified: <Check className="size-3.5 text-good" />,
  partial: <Minus className="size-3.5 text-warn" />,
  missing: <X className="size-3.5 text-bad" />,
  untested: <CircleDashed className="size-3.5 text-muted" />,
};

function Chat({ iv, setIv }: { iv: Interview; setIv: (iv: Interview) => void }) {
  const persona = useQuery({ queryKey: ["personas"], queryFn: api.personas, staleTime: Infinity }).data?.find(
    (p) => p.id === iv.personaId,
  );
  const [draft, setDraft] = useState("");
  const [thinking, setThinking] = useState(false);
  const [raw, setRaw] = useState("");
  const [spoken, setSpoken] = useState("");
  const [status, setStatus] = useState("");
  const [error, setError] = useState<string | null>(null);
  const request = useRef<string | null>(null);
  const bottom = useRef<HTMLDivElement>(null);
  const name = persona?.name ?? "Interviewer";

  useEffect(() => {
    bottom.current?.scrollIntoView({ behavior: "smooth" });
  }, [iv.transcript.length, raw]);

  useEffect(() => () => void (request.current && api.cancel(request.current)), []);

  const onEvent = (e: StreamEvent) => {
    if (e.type === "token") setRaw((r) => r + e.text);
    else if (e.type === "speech") setSpoken((r) => r + e.text);
    else setStatus(e.message);
  };

  async function respond() {
    setThinking(true);
    setRaw("");
    setSpoken("");
    setStatus("");
    setError(null);
    try {
      request.current = newRequestId();
      setIv(await api.interviewRespond(request.current, iv.id, onEvent));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setThinking(false);
      request.current = null;
    }
  }

  async function say(kind: "answer" | "hint_request" | "dont_know", text: string) {
    setError(null);
    try {
      setIv(await api.interviewSay(iv.id, kind, text));
      setDraft("");
    } catch (e) {
      setError(errorMessage(e));
      return;
    }
    await respond();
  }

  async function finish() {
    setThinking(true);
    setRaw("");
    setError(null);
    try {
      request.current = newRequestId();
      setIv(await api.finishInterview(request.current, iv.id, onEvent));
    } catch (e) {
      setError(errorMessage(e));
      setThinking(false);
    }
  }

  // The last question is done: write the report straight away.
  useEffect(() => {
    if (iv.status === "wrapping_up" && !thinking) finish();
  }, [iv.status]); // eslint-disable-line react-hooks/exhaustive-deps

  const pending = iv.transcript.at(-1)?.role === "candidate";
  const typed = spoken || streamedMessage(raw);
  const planned = plannedQuestions(iv.setup.minutes);

  return (
    <div className="flex h-full flex-col">
      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto flex max-w-3xl flex-col gap-3 px-6 py-6">
          {iv.transcript.map((t, i) =>
            t.role === "interviewer" ? (
              <div key={i} className="flex items-start gap-2.5">
                <Avatar name={name} size="size-8" />
                <div className="max-w-[85%]">
                  {t.kind === "question" && (
                    <div className="mb-1 flex items-center gap-1.5 text-[11px] font-semibold tracking-wide text-muted uppercase">
                      Question {iv.questions[t.question]?.order ?? "?"} of {planned}
                      <Badge tone={difficultyTone[iv.questions[t.question]?.difficulty ?? "medium"]}>
                        {iv.questions[t.question]?.difficulty}
                      </Badge>
                    </div>
                  )}
                  <div
                    className={`rounded-lg rounded-tl-none border px-3.5 py-2.5 ${t.kind === "question" ? "border-accent/40 bg-panel" : "border-line bg-panel"}`}
                  >
                    <Md>{t.text}</Md>
                  </div>
                  {t.kind === "hint" && (
                    <span className="mt-1 inline-flex items-center gap-1 text-[11px] text-warn">
                      <Lightbulb className="size-3" /> hint
                    </span>
                  )}
                </div>
              </div>
            ) : (
              <div key={i} className="flex justify-end">
                <div className="max-w-[85%] rounded-lg rounded-tr-none bg-accent-soft px-3.5 py-2.5 text-sm whitespace-pre-wrap">
                  {t.kind === "hint_request" ? <em className="text-muted">{t.text}</em> : t.text}
                </div>
              </div>
            ),
          )}
          {thinking && (
            <div className="flex items-start gap-2.5">
              <Avatar name={name} size="size-8" />
              <div className="max-w-[85%] rounded-lg rounded-tl-none border border-line bg-panel px-3.5 py-2.5 text-sm">
                {typed ? (
                  <span className="whitespace-pre-wrap">{typed}</span>
                ) : (
                  <span className="flex items-center gap-2 text-muted">
                    <Spinner />{" "}
                    {iv.status === "in_progress" ? status || `${name} is thinking…` : status || "Writing notes…"}
                  </span>
                )}
              </div>
            </div>
          )}
          <div ref={bottom} />
        </div>
      </div>

      <div className="border-t border-line bg-panel">
        <div className="mx-auto max-w-3xl px-6 py-3">
          <ErrorNote error={error} />
          {pending && !thinking && (
            <Button className="mt-2" onClick={respond}>
              {error ? "Try again" : "Get the interviewer's reply"}
            </Button>
          )}
          {error && iv.status === "wrapping_up" && (
            <Button className="mt-2" onClick={finish}>
              Try again
            </Button>
          )}
          {iv.status === "in_progress" && !pending && (
            <>
              <textarea
                autoFocus
                className={`${inputClass} mt-1 min-h-24 font-[inherit] leading-relaxed`}
                placeholder="Answer as you would out loud — explain your reasoning…"
                value={draft}
                disabled={thinking}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && draft.trim()) say("answer", draft);
                }}
              />
              <div className="mt-2 flex flex-wrap items-center gap-2">
                <Button variant="primary" disabled={!draft.trim() || thinking} onClick={() => say("answer", draft)}>
                  Answer
                </Button>
                <Button variant="ghost" disabled={thinking} onClick={() => say("hint_request", draft)}>
                  <Lightbulb className="size-4" /> Hint
                </Button>
                <Button variant="ghost" disabled={thinking} onClick={() => say("dont_know", "")}>
                  I don't know
                </Button>
                <span className="text-xs text-muted">Ctrl+Enter to send · hints cost a few points</span>
                <Button variant="ghost" className="ml-auto text-xs" disabled={thinking} onClick={finish}>
                  End interview
                </Button>
              </div>
            </>
          )}
          {iv.status !== "in_progress" && thinking && <StreamBox status={status || "Writing notes…"} text="" />}
        </div>
      </div>
    </div>
  );
}

function Rating({ label, value }: { label: string; value: number | null }) {
  if (value == null) return null;
  return (
    <div className="text-sm">
      <span className="text-muted">{label} </span>
      <span className="tracking-widest text-accent">{"●".repeat(value)}</span>
      <span className="tracking-widest text-line">{"●".repeat(5 - value)}</span>
    </div>
  );
}

function Report({ iv }: { iv: Interview }) {
  const navigate = useNavigate();
  const persona = useQuery({ queryKey: ["personas"], queryFn: api.personas, staleTime: Infinity }).data?.find(
    (p) => p.id === iv.personaId,
  );
  const r = iv.report!;
  const fp = iv.setup.bookFingerprint;
  const [showTranscript, setShowTranscript] = useState(false);

  return (
    <div className="mx-auto max-w-3xl px-6 py-6">
      <section className="mb-5 rounded-lg border border-line bg-panel p-5">
        <div className="mb-4 flex flex-wrap items-center gap-4">
          <div
            className={`flex size-16 items-center justify-center rounded-full text-xl font-bold ${{ good: "bg-good/15 text-good", warn: "bg-warn/15 text-warn", bad: "bg-bad/15 text-bad" }[scoreTone(r.overall)]}`}
          >
            {r.overall}
          </div>
          <div className="flex-1">
            <div className="flex items-center gap-2">
              <Badge tone={verdictTone[r.verdict]}>{verdictLabel[r.verdict]}</Badge>
              <span className="text-sm text-muted">{r.coverage}% of key points shown</span>
            </div>
            <div className="mt-1 flex flex-wrap gap-4">
              <Rating label="Depth" value={r.depth} />
              <Rating label="Clarity" value={r.clarity} />
            </div>
          </div>
          <Button variant="primary" onClick={() => navigate("/interview")}>
            New interview
          </Button>
        </div>
        {persona && (
          <div className="mb-2 flex items-center gap-2 text-xs text-muted">
            <Avatar name={persona.name} size="size-6" /> Notes from {persona.name}
          </div>
        )}
        <Md>{r.summary}</Md>
        <List title="Strengths" items={r.strengths} tone="text-good" />
        <List title="Study next" items={r.gaps} tone="text-warn" />
        {r.bestMoment && (
          <p className="mt-2 text-sm">
            <span className="text-xs font-semibold text-good">Best moment: </span>
            {r.bestMoment}
          </p>
        )}
        {r.weakestMoment && (
          <p className="mt-1 text-sm">
            <span className="text-xs font-semibold text-bad">Weakest moment: </span>
            {r.weakestMoment}
          </p>
        )}
        {fp && r.rereadPages.length > 0 && (
          <div className="mt-3 flex flex-wrap items-center gap-1 text-xs">
            <span className="font-semibold text-muted">Re-read:</span>
            {r.rereadPages.map((p) => (
              <Link
                key={p}
                to={`/book/${fp}?page=${p}`}
                className="rounded bg-accent-soft px-1.5 py-0.5 font-medium text-accent hover:underline"
              >
                p. {p}
              </Link>
            ))}
          </div>
        )}
        {iv.questions.some((q) => q.done && q.points.some((p) => p.status !== "verified")) && (
          <p className="mt-3 text-xs text-muted">
            Points you didn't fully show are now{" "}
            <Link to="/cards" className="text-accent hover:underline">
              review cards
            </Link>
            , first due tomorrow.
          </p>
        )}
      </section>

      <div className="flex flex-col gap-3">
        {iv.questions
          .map((q, i) => ({ q, i }))
          .filter(({ q }) => q.order != null)
          .sort((a, b) => a.q.order! - b.q.order!)
          .map(({ q, i }) => {
            const note = r.notes.find((n) => n.question === i)?.note;
            return (
              <section
                key={q.id}
                className={`rounded-lg border border-line bg-panel p-4 ${q.done ? "" : "opacity-60"}`}
              >
                <div className="mb-2 flex items-start gap-2">
                  <span className="text-sm font-semibold text-muted">{q.order}.</span>
                  <p className="flex-1 text-sm font-medium">{q.prompt}</p>
                  <Badge tone={difficultyTone[q.difficulty]}>{q.difficulty}</Badge>
                  {q.done && q.score != null ? (
                    <Badge tone={scoreTone(q.score)}>{q.score}</Badge>
                  ) : (
                    <Badge>not asked</Badge>
                  )}
                </div>
                {q.done && (
                  <>
                    <ul className="mb-2 flex flex-col gap-1">
                      {q.points.map((p, j) => (
                        <li key={j} className="flex items-start gap-2 text-sm">
                          <span className="mt-0.5">{pointIcon[p.status]}</span>
                          <span className={p.status === "verified" ? "" : "text-muted"}>{p.text}</span>
                        </li>
                      ))}
                    </ul>
                    {note && <p className="mb-2 text-sm italic">“{note}”</p>}
                    <div className="flex flex-wrap items-center gap-2 text-[11px] text-muted">
                      <span>
                        {q.turns} answer{q.turns === 1 ? "" : "s"}
                        {q.hints > 0 && ` · ${q.hints} hint${q.hints === 1 ? "" : "s"}`}
                      </span>
                      {fp &&
                        q.sources.slice(0, 3).map((s) => (
                          <Link
                            key={s.label}
                            to={`/book/${fp}?page=${s.pageStart}`}
                            className="inline-flex items-center gap-0.5 text-accent hover:underline"
                          >
                            <BookOpen className="size-3" /> p. {s.pageStart}
                          </Link>
                        ))}
                    </div>
                    <details className="mt-2 text-sm">
                      <summary className="cursor-pointer text-xs font-medium text-muted hover:text-ink">
                        Model answer
                      </summary>
                      <div className="mt-1 pl-3">
                        <Md>{q.reference}</Md>
                      </div>
                    </details>
                  </>
                )}
              </section>
            );
          })}
      </div>

      <button
        className="mt-5 text-xs font-medium text-muted hover:text-ink"
        onClick={() => setShowTranscript(!showTranscript)}
      >
        {showTranscript ? "Hide" : "Show"} full transcript ({iv.transcript.length} messages)
      </button>
      {showTranscript && (
        <div className="mt-2 flex flex-col gap-2 rounded-lg border border-line bg-panel p-4 text-sm">
          {iv.transcript.map((t, i) => (
            <div key={i}>
              <span className="text-xs font-semibold text-muted">
                {t.role === "interviewer" ? (persona?.name ?? "Interviewer") : "You"}:{" "}
              </span>
              <span className="whitespace-pre-wrap">{t.text}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export default function InterviewSessionPage() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const iv = useQuery({ queryKey: ["interview", id], queryFn: () => api.interview(id) });
  const persona = useQuery({ queryKey: ["personas"], queryFn: api.personas, staleTime: Infinity }).data?.find(
    (p) => p.id === iv.data?.personaId,
  );
  const setIv = (next: Interview) => {
    qc.setQueryData(["interview", id], next);
    qc.invalidateQueries({ queryKey: ["interviews"] });
    if (next.status === "completed") {
      for (const k of ["skills", "roadmap", "dashboard", "cards"]) qc.invalidateQueries({ queryKey: [k] });
    }
  };
  const d = iv.data;
  const asked = d ? d.questions.filter((q) => q.done).length : 0;
  const planned = d ? plannedQuestions(d.setup.minutes) : 0;
  const currentOrder = d ? (d.questions[d.current]?.order ?? asked) : 0;

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-2 border-b border-line bg-panel px-3 py-2">
        <Button variant="ghost" onClick={() => navigate("/interview")} aria-label="Back">
          <ArrowLeft className="size-4" />
        </Button>
        {persona && <Avatar name={persona.name} size="size-8" />}
        <div className="min-w-0">
          <div className="truncate text-sm font-semibold">
            {persona ? `${persona.name} · ${persona.role}` : "Interview"}
          </div>
          <div className="truncate text-xs text-muted">
            {d?.title} {d && `· ${d.setup.level}`}
          </div>
        </div>
        {d && d.status !== "completed" && (
          <div className="ml-auto flex items-center gap-2 text-xs text-muted">
            <span className="capitalize">{d.setup.mode}</span> · Question {Math.min(currentOrder, planned)} of {planned}
            <div className="h-1 w-32 overflow-hidden rounded bg-sunken">
              <div className="h-full bg-accent" style={{ width: `${(asked / planned) * 100}%` }} />
            </div>
          </div>
        )}
      </header>
      <div className="min-h-0 flex-1">
        {iv.isLoading && <Spinner className="m-6" />}
        {iv.error && (
          <div className="p-6">
            <ErrorNote error={errorMessage(iv.error)} />
          </div>
        )}
        {d &&
          (d.status === "completed" && d.report ? (
            <div className="h-full overflow-y-auto">
              <Report iv={d} />
            </div>
          ) : (
            <Chat iv={d} setIv={setIv} />
          ))}
      </div>
    </div>
  );
}
