import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "react-router";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { ArrowLeft, BookOpen, ChevronDown, ChevronRight, RotateCcw } from "lucide-react";
import { api, errorMessage, newRequestId, type Attempt, type Question } from "../ipc";
import { StreamBox } from "../components/StreamBox";
import { Badge, Button, ErrorNote, inputClass, scoreTone, Spinner } from "../ui";

const LETTERS = "ABCDEFGH";

const verdictTone = { correct: "good", partial: "warn", incorrect: "bad" } as const;

const nextStepText = {
  move_on: "You've got this chapter — move on to the next one.",
  review_weak_spots: "Mostly there. Re-read the pages below, then move on.",
  reread_chapter: "Re-read this chapter before moving on, then try the quiz again.",
};

function PageLinks({ fingerprint, pages }: { fingerprint: string | null; pages: number[] }) {
  if (!fingerprint || pages.length === 0) return null;
  return (
    <span className="inline-flex flex-wrap gap-1">
      {pages.map((p) => (
        <Link
          key={p}
          to={`/book/${fingerprint}?page=${p}`}
          className="inline-flex items-center gap-0.5 rounded bg-accent-soft px-1.5 py-0.5 text-[11px] font-medium text-accent hover:underline"
        >
          <BookOpen className="size-3" /> p. {p}
        </Link>
      ))}
    </span>
  );
}

export function Md({ children }: { children: string }) {
  return (
    <div className="prose-review text-sm">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{children}</ReactMarkdown>
    </div>
  );
}

export function List({ title, items, tone }: { title: string; items: string[]; tone: string }) {
  if (!items.length) return null;
  return (
    <div className="mt-2">
      <div className={`text-xs font-semibold ${tone}`}>{title}</div>
      <ul className="list-disc pl-5 text-sm">
        {items.map((x, i) => (
          <li key={i}>{x}</li>
        ))}
      </ul>
    </div>
  );
}

function Disclosure({ label, children }: { label: string; children: React.ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="mt-2">
      <button className="flex items-center gap-1 text-xs font-medium text-muted hover:text-ink" onClick={() => setOpen(!open)}>
        {open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
        {label}
      </button>
      {open && <div className="mt-1 pl-4">{children}</div>}
    </div>
  );
}

function answerText(q: Question) {
  if (q.type === "mcq" && q.answer != null && q.choices) {
    const i = Number(q.answer);
    return Number.isInteger(i) && q.choices[i] ? `${LETTERS[i]}. ${q.choices[i]}` : q.answer;
  }
  return q.answer?.trim() || "(no answer)";
}

function QuestionReview({
  q,
  n,
  fingerprint,
  onRegrade,
  regrading,
}: {
  q: Question;
  n: number;
  fingerprint: string | null;
  onRegrade: () => void;
  regrading: boolean;
}) {
  const g = q.grade;
  return (
    <div className="rounded-lg border border-line bg-panel p-4">
      <div className="mb-2 flex items-start gap-2">
        <span className="text-sm font-semibold text-muted">{n}.</span>
        <p className="flex-1 text-sm font-medium">{q.prompt}</p>
        {g ? (
          <Badge tone={verdictTone[g.verdict]}>
            {g.verdict} · {g.score}
          </Badge>
        ) : (
          <Badge>not graded</Badge>
        )}
      </div>
      <div className="mb-2 rounded-md bg-sunken px-3 py-2 text-sm">
        <span className="text-xs font-semibold text-muted">Your answer: </span>
        <span className="whitespace-pre-wrap">{answerText(q)}</span>
      </div>
      {g && (
        <>
          <Md>{g.feedback}</Md>
          <List title="What you got right" items={g.correctPoints} tone="text-good" />
          <List title="What was missing" items={g.missingPoints} tone="text-warn" />
          <List title="Misconceptions" items={g.misconceptions} tone="text-bad" />
          {g.rereadPages.length > 0 && (
            <div className="mt-2 flex items-center gap-2 text-xs">
              <span className="font-semibold text-muted">Re-read:</span>
              <PageLinks fingerprint={fingerprint} pages={g.rereadPages} />
            </div>
          )}
        </>
      )}
      <Disclosure label="Model answer">
        <Md>{q.referenceAnswer}</Md>
        {q.type === "mcq" && q.choices && q.correctChoice != null && (
          <p className="text-xs text-muted">
            Correct choice: {LETTERS[q.correctChoice]}. {q.choices[q.correctChoice]}
          </p>
        )}
      </Disclosure>
      <Disclosure label={`Source excerpts (${q.sources.length})`}>
        {q.sources.map((s) => (
          <div key={s.label} className="mb-2">
            <PageLinks fingerprint={fingerprint} pages={[s.pageStart]} />
            <pre className="mt-1 max-h-48 overflow-y-auto rounded bg-sunken p-2 font-sans text-xs whitespace-pre-wrap text-muted">
              {s.excerpt}
            </pre>
          </div>
        ))}
      </Disclosure>
      {q.type !== "mcq" && q.answer && (
        <div className="mt-2">
          <Button variant="ghost" className="text-xs" busy={regrading} onClick={onRegrade}>
            <RotateCcw className="size-3.5" /> Grade again
          </Button>
        </div>
      )}
    </div>
  );
}

export function Review({ attempt, setAttempt }: { attempt: Attempt; setAttempt: (a: Attempt) => void }) {
  const o = attempt.overall;
  const [regrading, setRegrading] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function regrade(q: Question) {
    setRegrading(q.id);
    setError(null);
    try {
      await api.gradeAnswer(newRequestId(), attempt.id, q.id, q.answer ?? "", () => {});
      setAttempt(await api.finishQuiz(newRequestId(), attempt.id, () => {}));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setRegrading(null);
    }
  }

  return (
    <div className="mx-auto max-w-3xl px-6 py-6">
      {o && (
        <section className="mb-6 rounded-lg border border-line bg-panel p-5">
          <div className="mb-3 flex items-center gap-4">
            <div
              className={`flex size-16 items-center justify-center rounded-full text-xl font-bold ${{ good: "bg-good/15 text-good", warn: "bg-warn/15 text-warn", bad: "bg-bad/15 text-bad" }[scoreTone(o.score)]}`}
            >
              {o.score}
            </div>
            <div>
              <h2 className="text-lg font-semibold">{attempt.chapterTitle}</h2>
              <p className="text-sm text-muted">{nextStepText[o.nextStep] ?? ""}</p>
            </div>
          </div>
          <Md>{o.summary}</Md>
          <List title="Strengths" items={o.strengths} tone="text-good" />
          <List title="Focus next" items={o.focusNext} tone="text-warn" />
          {o.rereadPages.length > 0 && (
            <div className="mt-3 flex flex-wrap items-center gap-2 text-xs">
              <span className="font-semibold text-muted">Pages to re-read:</span>
              <PageLinks fingerprint={attempt.bookFingerprint} pages={o.rereadPages} />
            </div>
          )}
        </section>
      )}
      <ErrorNote error={error} />
      <div className="flex flex-col gap-3">
        {attempt.questions.map((q, i) => (
          <QuestionReview
            key={q.id}
            q={q}
            n={i + 1}
            fingerprint={attempt.bookFingerprint}
            onRegrade={() => regrade(q)}
            regrading={regrading === q.id}
          />
        ))}
      </div>
    </div>
  );
}

/** Grades answers one at a time in the background while the learner keeps going. */
export function useGradingQueue(attemptId: string, onGraded: (a: Attempt) => void) {
  const queue = useRef<{ questionId: string; answer: string }[]>([]);
  const running = useRef(false);
  const currentRequest = useRef<string | null>(null);
  const [pending, setPending] = useState(0);
  const [stream, setStream] = useState("");
  const [errors, setErrors] = useState<Record<string, string>>({});
  const idleWaiters = useRef<(() => void)[]>([]);

  async function pump() {
    if (running.current) return;
    running.current = true;
    while (queue.current.length) {
      const job = queue.current[0];
      setStream("");
      currentRequest.current = newRequestId();
      try {
        const a = await api.gradeAnswer(currentRequest.current, attemptId, job.questionId, job.answer, (e) => {
          if (e.type === "token") setStream((s) => s + e.text);
        });
        onGraded(a);
        setErrors(({ [job.questionId]: _, ...rest }) => rest);
      } catch (e) {
        setErrors((x) => ({ ...x, [job.questionId]: errorMessage(e) }));
      }
      queue.current.shift();
      setPending(queue.current.length);
    }
    running.current = false;
    currentRequest.current = null;
    idleWaiters.current.splice(0).forEach((f) => f());
  }

  return {
    pending,
    stream,
    errors,
    enqueue(questionId: string, answer: string) {
      queue.current.push({ questionId, answer });
      setPending(queue.current.length);
      pump();
    },
    idle: () => (running.current ? new Promise<void>((r) => idleWaiters.current.push(r)) : Promise.resolve()),
    cancel() {
      queue.current = [];
      if (currentRequest.current) api.cancel(currentRequest.current);
    },
  };
}

function QuestionFlow({ attempt, setAttempt }: { attempt: Attempt; setAttempt: (a: Attempt) => void }) {
  const firstOpen = attempt.questions.findIndex((q) => q.answer == null);
  const [index, setIndex] = useState(firstOpen === -1 ? attempt.questions.length : firstOpen);
  const [draft, setDraft] = useState("");
  const [finishing, setFinishing] = useState(false);
  const [finishStatus, setFinishStatus] = useState("");
  const [finishText, setFinishText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const grading = useGradingQueue(attempt.id, setAttempt);

  useEffect(() => {
    // Resume: answers saved before the app closed but never graded
    attempt.questions.filter((x) => x.answer != null && !x.grade).forEach((x) => grading.enqueue(x.id, x.answer!));
    return () => grading.cancel();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const q = attempt.questions[index];
  const total = attempt.questions.length;

  function submit(answer: string) {
    grading.enqueue(q.id, answer);
    setDraft("");
    setIndex(index + 1);
  }

  async function finish() {
    setFinishing(true);
    setError(null);
    try {
      setFinishStatus("Finishing grading…");
      await grading.idle();
      const a = await api.finishQuiz(newRequestId(), attempt.id, (e) => {
        if (e.type === "status") setFinishStatus(e.message);
        else setFinishText((t) => t + e.text);
      });
      setAttempt(a);
    } catch (e) {
      setError(errorMessage(e));
      setFinishing(false);
    }
  }

  if (!q) {
    return (
      <div className="mx-auto max-w-2xl px-6 py-10">
        <h2 className="mb-2 text-lg font-semibold">All {total} questions answered</h2>
        <p className="mb-4 text-sm text-muted">
          {grading.pending > 0
            ? `Still grading ${grading.pending} answer${grading.pending === 1 ? "" : "s"}…`
            : "Ready for your review."}
        </p>
        {Object.entries(grading.errors).map(([id, msg]) => (
          <ErrorNote key={id} error={`Question ${attempt.questions.findIndex((x) => x.id === id) + 1}: ${msg}`} />
        ))}
        {finishing ? (
          <StreamBox status={finishStatus} text={grading.pending ? grading.stream : finishText} />
        ) : (
          <Button variant="primary" onClick={finish}>
            Show my review
          </Button>
        )}
        <div className="mt-3">
          <ErrorNote error={error} />
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-2xl px-6 py-8">
      <div className="mb-4 flex items-center gap-3">
        <span className="text-xs font-medium text-muted">
          Question {index + 1} of {total}
        </span>
        <div className="h-1 flex-1 overflow-hidden rounded bg-sunken">
          <div className="h-full bg-accent" style={{ width: `${(index / total) * 100}%` }} />
        </div>
        {grading.pending > 0 && (
          <span className="flex items-center gap-1.5 text-xs text-muted">
            <Spinner /> grading {grading.pending}
          </span>
        )}
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
        </div>
      ) : (
        <>
          <textarea
            autoFocus
            className={`${inputClass} min-h-40 font-[inherit] leading-relaxed`}
            placeholder={q.type === "explain" ? "Explain it in your own words…" : "Your answer (1–3 sentences)…"}
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
      <p className="mt-6 text-xs text-muted">Answers are graded in the background while you continue.</p>
    </div>
  );
}

export default function QuizPage() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const attempt = useQuery({ queryKey: ["attempt", id], queryFn: () => api.attempt(id) });
  const setAttempt = (a: Attempt) => {
    qc.setQueryData(["attempt", id], a);
    qc.invalidateQueries({ queryKey: ["attempts"] });
    qc.invalidateQueries({ queryKey: ["books"] });
    qc.invalidateQueries({ queryKey: ["skills"] });
    qc.invalidateQueries({ queryKey: ["roadmap"] });
  };

  return (
    <div className="h-full overflow-y-auto">
      <header className="sticky top-0 z-10 flex items-center gap-2 border-b border-line bg-panel px-3 py-2">
        <Button
          variant="ghost"
          onClick={() =>
            attempt.data?.bookFingerprint ? navigate(`/book/${attempt.data.bookFingerprint}`) : navigate("/history")
          }
          aria-label="Back"
        >
          <ArrowLeft className="size-4" />
        </Button>
        <div className="min-w-0">
          <div className="truncate text-sm font-semibold">{attempt.data?.chapterTitle ?? "Quiz"}</div>
          <div className="truncate text-xs text-muted">
            {attempt.data?.model.question ? `Questions by ${attempt.data.model.question.model}` : " "}
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
          <Review attempt={attempt.data} setAttempt={setAttempt} />
        ) : (
          <QuestionFlow attempt={attempt.data} setAttempt={setAttempt} />
        ))}
    </div>
  );
}
