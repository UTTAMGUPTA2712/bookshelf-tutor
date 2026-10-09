import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router";
import { BookOpen, Pencil, Plus, Search, Trash2 } from "lucide-react";
import { api, errorMessage, type Card } from "../ipc";
import { Md } from "./Quiz";
import { Badge, Button, Empty, ErrorNote, Field, inputClass, Modal, Spinner } from "../ui";

const RATINGS = [
  { n: 1, label: "Again", cls: "border-bad/40 text-bad" },
  { n: 2, label: "Hard", cls: "border-warn/40 text-warn" },
  { n: 3, label: "Good", cls: "border-good/40 text-good" },
  { n: 4, label: "Easy", cls: "border-accent/40 text-accent" },
] as const;

function Source({ card }: { card: Card }) {
  if (!card.bookFingerprint) return null;
  return (
    <Link
      to={`/book/${card.bookFingerprint}${card.page ? `?page=${card.page}` : ""}`}
      className="inline-flex items-center gap-1 text-xs text-muted hover:text-accent hover:underline"
    >
      <BookOpen className="size-3" />
      {card.bookTitle ?? "Book"}
      {card.page ? `, p. ${card.page}` : ""}
    </Link>
  );
}

/** Create or edit a card. */
export function CardEditor({
  initial,
  onClose,
  onSaved,
}: {
  initial: Partial<Card> & { front: string; back: string };
  onClose: () => void;
  onSaved?: () => void;
}) {
  const qc = useQueryClient();
  const [front, setFront] = useState(initial.front);
  const [back, setBack] = useState(initial.back);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save() {
    setBusy(true);
    setError(null);
    try {
      if (initial.id) await api.updateCard(initial.id, front, back);
      else
        await api.createCard({
          front,
          back,
          bookFingerprint: initial.bookFingerprint ?? null,
          page: initial.page ?? null,
        });
      qc.invalidateQueries({ queryKey: ["cards"] });
      qc.invalidateQueries({ queryKey: ["dashboard"] });
      onSaved?.();
      onClose();
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  }

  return (
    <Modal
      title={initial.id ? "Edit card" : "New card"}
      onClose={onClose}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" busy={busy} disabled={!front.trim() || !back.trim()} onClick={save}>
            Save card
          </Button>
        </>
      }
    >
      <Field label="Front (question)">
        <textarea autoFocus className={`${inputClass} min-h-20`} value={front} onChange={(e) => setFront(e.target.value)} />
      </Field>
      <Field label="Back (answer)" hint="Markdown works. Keep it to one fact.">
        <textarea className={`${inputClass} min-h-28`} value={back} onChange={(e) => setBack(e.target.value)} />
      </Field>
      {!initial.id && initial.bookFingerprint && (
        <p className="text-xs text-muted">Linked to page {initial.page} of this book. New cards are due right away.</p>
      )}
      <ErrorNote error={error} />
    </Modal>
  );
}

function Review() {
  const qc = useQueryClient();
  const queue = useQuery({ queryKey: ["cards", "due"], queryFn: api.cardsDue });
  const [shown, setShown] = useState(false);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const card = queue.data?.cards[0];

  async function rate(n: 1 | 2 | 3 | 4) {
    if (!card || busy) return;
    setBusy(true);
    setError(null);
    try {
      await api.reviewCard(card.id, n);
      setDone((d) => d + 1);
      setShown(false);
      await qc.invalidateQueries({ queryKey: ["cards"] });
      qc.invalidateQueries({ queryKey: ["dashboard"] });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLInputElement) return;
      if (!shown && (e.key === " " || e.key === "Enter")) {
        e.preventDefault();
        setShown(true);
      } else if (shown && ["1", "2", "3", "4"].includes(e.key)) {
        rate(Number(e.key) as 1 | 2 | 3 | 4);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  if (queue.isLoading) return <Spinner className="m-6" />;
  if (queue.error) return <ErrorNote error={errorMessage(queue.error)} />;
  if (!card) {
    const more = (queue.data?.dueNow ?? 0) > 0;
    return (
      <Empty title={done ? `Done — ${done} card${done === 1 ? "" : "s"} reviewed` : "Nothing to review right now"}>
        {more
          ? `You've reached today's limit of ${queue.data?.dailyLimit} cards. The rest wait until tomorrow.`
          : "Cards from your quiz mistakes show up here the day after the quiz."}
      </Empty>
    );
  }

  const remaining = Math.min(queue.data!.dueNow, queue.data!.cards.length);
  return (
    <div className="mx-auto max-w-2xl py-6">
      <div className="mb-3 flex items-center gap-2 text-xs text-muted">
        <span>{remaining} left today</span>
        {done > 0 && <span>· {done} done</span>}
        <Badge>{card.state === "new" ? "new" : card.state === "review" ? `review · ${card.reps}×` : "again"}</Badge>
        <span className="ml-auto">
          <Source card={card} />
        </span>
      </div>
      <div className="rounded-lg border border-line bg-panel p-6">
        <div className="text-base leading-relaxed font-medium">
          <Md>{card.front}</Md>
        </div>
        {shown && (
          <div className="mt-4 border-t border-line pt-4">
            <Md>{card.back}</Md>
          </div>
        )}
      </div>
      <ErrorNote error={error} />
      <div className="mt-4">
        {shown ? (
          <div className="grid grid-cols-4 gap-2">
            {RATINGS.map((r) => (
              <button
                key={r.n}
                disabled={busy}
                onClick={() => rate(r.n)}
                className={`rounded-lg border bg-panel px-3 py-2 text-sm font-medium hover:bg-sunken disabled:opacity-50 ${r.cls}`}
              >
                {r.label}
                <span className="block text-[11px] font-normal text-muted">
                  {card.preview[r.n - 1]} · {r.n}
                </span>
              </button>
            ))}
          </div>
        ) : (
          <Button variant="primary" className="w-full py-2" onClick={() => setShown(true)}>
            Show answer <span className="text-xs opacity-75">(Space)</span>
          </Button>
        )}
      </div>
    </div>
  );
}

function dueLabel(c: Card) {
  const ms = new Date(c.due).getTime() - Date.now();
  if (ms <= 0) return "due now";
  const days = Math.round(ms / 86_400_000);
  return days < 1 ? "due today" : `in ${days} day${days === 1 ? "" : "s"}`;
}

function Browse() {
  const qc = useQueryClient();
  const [q, setQ] = useState("");
  const cards = useQuery({ queryKey: ["cards", "list", q], queryFn: () => api.cards(q) });
  const [editing, setEditing] = useState<Card | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function remove(c: Card) {
    setError(null);
    try {
      await api.deleteCard(c.id);
      qc.invalidateQueries({ queryKey: ["cards"] });
      qc.invalidateQueries({ queryKey: ["dashboard"] });
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  return (
    <div className="py-4">
      <div className="relative mb-3 max-w-sm">
        <Search className="absolute top-2 left-2.5 size-4 text-muted" />
        <input className={`${inputClass} pl-8`} placeholder="Search cards…" value={q} onChange={(e) => setQ(e.target.value)} />
      </div>
      <ErrorNote error={error} />
      {cards.isLoading && <Spinner />}
      {cards.data?.length === 0 && (
        <Empty title={q ? "No matching cards" : "No cards yet"}>
          Cards are made from the questions you miss in chapter quizzes, or from text you select in the reader.
        </Empty>
      )}
      <ul className="flex flex-col gap-2">
        {cards.data?.map((c) => (
          <li key={c.id} className="group rounded-lg border border-line bg-panel p-3">
            <div className="flex items-start gap-2">
              <div className="min-w-0 flex-1">
                <p className="text-sm font-medium">{c.front}</p>
                <p className="mt-1 line-clamp-2 text-sm text-muted">{c.back}</p>
              </div>
              <div className="flex shrink-0 gap-1 opacity-0 group-hover:opacity-100">
                <button className="rounded p-1 text-muted hover:bg-sunken" onClick={() => setEditing(c)} aria-label="Edit card">
                  <Pencil className="size-3.5" />
                </button>
                <button className="rounded p-1 text-muted hover:bg-sunken hover:text-bad" onClick={() => remove(c)} aria-label="Delete card">
                  <Trash2 className="size-3.5" />
                </button>
              </div>
            </div>
            <div className="mt-2 flex flex-wrap items-center gap-2 text-[11px] text-muted">
              <Badge>{dueLabel(c)}</Badge>
              {c.reps > 0 && (
                <span>
                  reviewed {c.reps}× {c.lapses > 0 && `· forgot ${c.lapses}×`}
                </span>
              )}
              <Source card={c} />
            </div>
          </li>
        ))}
      </ul>
      {editing && <CardEditor initial={editing} onClose={() => setEditing(null)} />}
    </div>
  );
}

export default function CardsPage() {
  const qc = useQueryClient();
  const [tab, setTab] = useState<"review" | "browse">("review");
  const [creating, setCreating] = useState(false);
  const prefs = useQuery({ queryKey: ["prefs"], queryFn: api.prefs });

  async function setLimit(n: number) {
    qc.setQueryData(["prefs"], await api.setPrefs({ dailyCards: n }));
    qc.invalidateQueries({ queryKey: ["cards"] });
  }

  return (
    <div className="h-full overflow-y-auto">
      <header className="sticky top-0 z-10 flex flex-wrap items-center gap-3 border-b border-line bg-bg/95 px-6 py-3 backdrop-blur">
        <h1 className="text-lg font-semibold">Cards</h1>
        <div className="flex gap-1 rounded-lg bg-sunken p-1">
          {(["review", "browse"] as const).map((t) => (
            <button
              key={t}
              onClick={() => setTab(t)}
              className={`rounded-md px-3 py-1 text-sm ${t === tab ? "bg-panel font-medium shadow-sm" : "text-muted hover:text-ink"}`}
            >
              {t === "review" ? "Review" : "All cards"}
            </button>
          ))}
        </div>
        <label className="ml-auto flex items-center gap-2 text-sm text-muted">
          Daily limit
          <select
            className={`${inputClass} w-auto`}
            value={prefs.data?.dailyCards ?? 50}
            onChange={(e) => setLimit(Number(e.target.value))}
          >
            {[10, 20, 30, 50, 100, 200].map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
        </label>
        <Button onClick={() => setCreating(true)}>
          <Plus className="size-4" /> New card
        </Button>
      </header>
      <div className="px-6">{tab === "review" ? <Review /> : <Browse />}</div>
      {creating && <CardEditor initial={{ front: "", back: "" }} onClose={() => setCreating(false)} />}
    </div>
  );
}
