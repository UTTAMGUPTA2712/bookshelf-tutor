import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "react-router";
import { ArrowLeft, ArrowRight } from "lucide-react";
import { api, errorMessage, newRequestId, type AssessmentStep, type Attempt, type StreamEvent } from "../ipc";
import { StreamBox } from "../components/StreamBox";
import { Review, useGradingQueue } from "./Quiz";
import { Badge, Button, ErrorNote, inputClass, scoreTone, Spinner } from "../ui";

const LETTERS = "ABCDEFGH";
const difficultyTone = { easy: "good", medium: "accent", hard: "warn" } as const;

function useTopicLabels(track: string | undefined) {
  const map = useQuery({ queryKey: ["skills", track], queryFn: () => api.skills(track!), enabled: !!track });
  return new Map(map.data?.topics.map((t) => [t.id, t.label]));
}

function Flow({ attempt, setAttempt }: { attempt: Attempt; setAttempt: (a: Attempt) => void }) {
  const labels = useTopicLabels(attempt.model.track);
  const [step, setStep] = useState<AssessmentStep | null>(null);
  const [status, setStatus] = useState("");
  const [stream, setStream] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [finishing, setFinishing] = useState(false);
  const grading = useGradingQueue(attempt.id, () => {});
  const prefetch = useRef<{ id: string; done: Promise<void> } | null>(null);
  const nextRequest = useRef<string | null>(null);

  function startPrefetch() {
    const id = newRequestId();
    prefetch.current = { id, done: api.assessmentPrefetch(id, attempt.id).catch(() => {}) };
  }

  const onEvent = (e: StreamEvent) => {
    if (e.type === "status") setStatus(e.message);
    else setStream((s) => s + e.text);
  };

  async function next() {
    setBusy(true);
    setError(null);
    setStream("");
    setStatus("Preparing the next question…");
    try {
      // Don't write the same topic's questions twice.
      await prefetch.current?.done;
      nextRequest.current = newRequestId();
      const s = await api.assessmentNext(nextRequest.current, attempt.id, onEvent);
      if (!s.questionId) {
        await finish();
        return;
      }
      setStep(s);
      setDraft("");
      startPrefetch();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function finish() {
    setFinishing(true);
    setError(null);
    setStream("");
    try {
      setStatus("Finishing grading…");
      await grading.idle();
      if (prefetch.current) api.cancel(prefetch.current.id);
      const a = await api.finishQuiz(newRequestId(), attempt.id, onEvent);
      setAttempt(a);
    } catch (e) {
      setError(errorMessage(e));
      setFinishing(false);
    }
  }

  useEffect(() => {
    next();
    return () => {
      grading.cancel();
      if (prefetch.current) api.cancel(prefetch.current.id);
      if (nextRequest.current) api.cancel(nextRequest.current);
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const q = step?.attempt.questions.find((x) => x.id === step.questionId);

  async function submit(answer: string) {
    if (!q) return;
    setBusy(true);
    try {
      if (q.type === "mcq" || !answer.trim()) {
        // Graded instantly on this machine; the next question depends on it.
        await api.gradeAnswer(newRequestId(), attempt.id, q.id, answer, () => {});
      } else {
        await api.saveAnswer(attempt.id, q.id, answer);
        grading.enqueue(q.id, answer);
      }
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
      return;
    }
    await next();
  }

  if (finishing) {
    return (
      <div className="mx-auto max-w-2xl px-6 py-10">
        <h2 className="mb-3 text-lg font-semibold">Working out your skill map</h2>
        <StreamBox status={status} text={grading.pending ? grading.stream : stream} />
        <div className="mt-3">
          <ErrorNote error={error} />
        </div>
        {error && (
          <Button className="mt-3" onClick={finish}>
            Try again
          </Button>
        )}
      </div>
    );
  }

  const done = step?.topicsDone ?? 0;
  const total = step?.topicsTotal ?? 0;

  return (
    <div className="mx-auto max-w-2xl px-6 py-8">
      <div className="mb-4 flex items-center gap-3">
        <span className="text-xs font-medium text-muted">
          Topic {Math.max(1, done)} of {total || "…"}
        </span>
        <div className="h-1 flex-1 overflow-hidden rounded bg-sunken">
          <div className="h-full bg-accent" style={{ width: total ? `${(done / total) * 100}%` : "0%" }} />
        </div>
        {grading.pending > 0 && (
          <span className="flex items-center gap-1.5 text-xs text-muted">
            <Spinner /> grading {grading.pending}
          </span>
        )}
        <Button variant="ghost" className="text-xs" disabled={finishing} onClick={finish}>
          End test now
        </Button>
      </div>

      {busy && !q && <StreamBox status={status} text={stream} />}
      {error && (
        <div className="mb-3">
          <ErrorNote error={error} />
          <Button className="mt-2" onClick={next}>
            Try again
          </Button>
        </div>
      )}

      {q && (
        <div className={busy ? "pointer-events-none opacity-60" : ""}>
          <div className="mb-2 flex items-center gap-2">
            <span className="text-sm font-medium text-muted">{labels.get(q.topic ?? "") ?? q.topic}</span>
            {q.difficulty && <Badge tone={difficultyTone[q.difficulty]}>{q.difficulty}</Badge>}
          </div>
          <h2 className="mb-4 text-base leading-relaxed font-medium">{q.prompt}</h2>
          {q.type === "mcq" && q.choices ? (
            <div className="flex flex-col gap-2">
              {q.choices.map((c, i) => (
                <button
                  key={i}
                  className="flex items-start gap-3 rounded-lg border border-line bg-panel px-4 py-3 text-left text-sm hover:border-accent"
                  onClick={() => submit(String(i))}
                >
                  <span className="font-semibold text-muted">{LETTERS[i]}</span>
                  <span>{c}</span>
                </button>
              ))}
              <Button variant="ghost" className="self-start" onClick={() => submit("")}>
                I don't know
              </Button>
            </div>
          ) : (
            <>
              <textarea
                autoFocus
                className={`${inputClass} min-h-32 font-[inherit] leading-relaxed`}
                placeholder="Your answer (1–3 sentences)…"
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && draft.trim()) submit(draft);
                }}
              />
              <div className="mt-3 flex items-center gap-2">
                <Button variant="primary" disabled={!draft.trim()} onClick={() => submit(draft)}>
                  Submit answer
                </Button>
                <Button variant="ghost" onClick={() => submit("")}>
                  I don't know
                </Button>
                <span className="ml-auto text-xs text-muted">Ctrl+Enter to submit</span>
              </div>
            </>
          )}
          {busy && (
            <div className="mt-4">
              <StreamBox status={status} text={stream} />
            </div>
          )}
        </div>
      )}
      <p className="mt-6 text-xs text-muted">
        Answer honestly — “I don't know” gives a better roadmap than a guess. The next topic's questions are written
        while you answer.
      </p>
    </div>
  );
}

function Results({ attempt, setAttempt }: { attempt: Attempt; setAttempt: (a: Attempt) => void }) {
  const labels = useTopicLabels(attempt.model.track);
  const byTopic = new Map<string, { scores: number[] }>();
  for (const q of attempt.questions) {
    if (!q.topic || !q.grade) continue;
    const e = byTopic.get(q.topic) ?? { scores: [] };
    e.scores.push(q.grade.score);
    byTopic.set(q.topic, e);
  }
  return (
    <>
      <div className="mx-auto max-w-3xl px-6 pt-6">
        <section className="rounded-lg border border-line bg-panel p-5">
          <div className="mb-3 flex flex-wrap items-center gap-3">
            <h2 className="text-base font-semibold">Topics in this test</h2>
            <Link to="/skills" className="ml-auto flex items-center gap-1 text-sm text-accent hover:underline">
              Skill map <ArrowRight className="size-3.5" />
            </Link>
            <Link to="/roadmap" className="flex items-center gap-1 text-sm text-accent hover:underline">
              Roadmap <ArrowRight className="size-3.5" />
            </Link>
          </div>
          <div className="flex flex-wrap gap-1.5">
            {[...byTopic.entries()].map(([t, e]) => {
              const avg = Math.round(e.scores.reduce((a, b) => a + b, 0) / e.scores.length);
              return (
                <Badge key={t} tone={scoreTone(avg)}>
                  {labels.get(t) ?? t} · {e.scores.length} q
                </Badge>
              );
            })}
          </div>
        </section>
      </div>
      <Review attempt={attempt} setAttempt={setAttempt} />
    </>
  );
}

export default function AssessmentPage() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const attempt = useQuery({ queryKey: ["attempt", id], queryFn: () => api.attempt(id) });
  const setAttempt = (a: Attempt) => {
    qc.setQueryData(["attempt", id], a);
    qc.invalidateQueries({ queryKey: ["attempts"] });
    qc.invalidateQueries({ queryKey: ["skills"] });
    qc.invalidateQueries({ queryKey: ["roadmap"] });
  };

  return (
    <div className="h-full overflow-y-auto">
      <header className="sticky top-0 z-10 flex items-center gap-2 border-b border-line bg-panel px-3 py-2">
        <Button variant="ghost" onClick={() => navigate("/skills")} aria-label="Back">
          <ArrowLeft className="size-4" />
        </Button>
        <div className="min-w-0">
          <div className="truncate text-sm font-semibold">{attempt.data?.chapterTitle ?? "Skill test"}</div>
          <div className="truncate text-xs text-muted">
            {attempt.data?.model.question ? `Questions by ${attempt.data.model.question.model}` : " "}
          </div>
        </div>
      </header>
      {attempt.isLoading && <Spinner className="m-6" />}
      {attempt.error && (
        <div className="p-6">
          <ErrorNote error={errorMessage(attempt.error)} />
        </div>
      )}
      {attempt.data &&
        (attempt.data.status === "completed" ? (
          <Results attempt={attempt.data} setAttempt={setAttempt} />
        ) : (
          <Flow attempt={attempt.data} setAttempt={setAttempt} />
        ))}
    </div>
  );
}
