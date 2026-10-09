import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "react-router";
import { api, errorMessage } from "../ipc";
import { useBooks } from "./Library";
import { Badge, Button, Empty, ErrorNote, Field, formatDate, inputClass, Spinner } from "../ui";

function NewLesson() {
  const navigate = useNavigate();
  const books = useBooks();
  const [fp, setFp] = useState("");
  const [idx, setIdx] = useState<number | null>(null);
  const chapters = useQuery({ queryKey: ["chapters", fp], queryFn: () => api.bookChapters(fp), enabled: !!fp });
  const usable = (books.data ?? []).filter((b) => b.path && b.status !== "outdated");

  return (
    <section className="rounded-lg border border-line bg-panel p-5">
      <h2 className="mb-1 text-base font-semibold">Learn a chapter</h2>
      <p className="mb-4 text-sm text-muted">
        Step 1: a concise summary with every keyword. Step 2: each topic taught in plain words, then in industry terms,
        then a recap. Or press <b>Teach me</b> while reading.
      </p>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label="Book">
          <select
            className={inputClass}
            value={fp}
            onChange={(e) => {
              setFp(e.target.value);
              setIdx(null);
            }}
          >
            <option value="">Choose a book…</option>
            {usable.map((b) => (
              <option key={b.fingerprint} value={b.fingerprint}>
                {b.title}
              </option>
            ))}
          </select>
        </Field>
        <Field
          label="Chapter"
          hint={fp && chapters.data?.length === 0 ? "Open this book once so the app can read its chapters." : undefined}
        >
          <select
            className={inputClass}
            value={idx ?? ""}
            disabled={!chapters.data?.length}
            onChange={(e) => setIdx(e.target.value === "" ? null : Number(e.target.value))}
          >
            <option value="">Choose a chapter…</option>
            {chapters.data?.map((c) => (
              <option key={c.idx} value={c.idx}>
                {c.title} (p. {c.startPage})
              </option>
            ))}
          </select>
        </Field>
      </div>
      <Button variant="primary" disabled={!fp || idx == null} onClick={() => navigate(`/lesson/${fp}/${idx}`)}>
        Open lesson
      </Button>
    </section>
  );
}

export default function LessonsPage() {
  const navigate = useNavigate();
  const list = useQuery({ queryKey: ["lessons"], queryFn: api.lessons });

  return (
    <div className="h-full overflow-y-auto">
      <header className="sticky top-0 z-10 border-b border-line bg-bg/95 px-6 py-3 backdrop-blur">
        <h1 className="text-lg font-semibold">Lessons</h1>
      </header>
      <div className="mx-auto max-w-5xl px-6 py-5">
        <NewLesson />
        <h2 className="mt-8 mb-3 text-base font-semibold">Your lessons</h2>
        {list.isLoading && <Spinner />}
        {list.error && <ErrorNote error={errorMessage(list.error)} />}
        {list.data?.length === 0 && (
          <Empty title="No lessons yet">Lessons are saved once written, and travel with your progress export.</Empty>
        )}
        {list.data && list.data.length > 0 && (
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-line text-left text-xs text-muted">
                <th className="py-2 font-medium">Chapter</th>
                <th className="py-2 font-medium">Book</th>
                <th className="py-2 font-medium">Keywords</th>
                <th className="py-2 font-medium">Topics taught</th>
                <th className="py-2 font-medium">Updated</th>
              </tr>
            </thead>
            <tbody>
              {list.data.map((l) => (
                <tr
                  key={l.id}
                  className="cursor-pointer border-b border-line hover:bg-panel"
                  onClick={() => navigate(`/lesson/${l.fingerprint}/${l.chapterIdx}`)}
                >
                  <td className="py-2 pr-3">{l.chapterTitle}</td>
                  <td className="py-2 pr-3 text-muted">{l.bookTitle}</td>
                  <td className="py-2 pr-3 tabular-nums">{l.terms}</td>
                  <td className="py-2 pr-3">
                    <Badge tone={l.topicsTaught === l.topics && l.topics > 0 ? "good" : "neutral"}>
                      {l.topicsTaught}/{l.topics}
                    </Badge>
                  </td>
                  <td className="py-2 text-muted">{formatDate(l.updatedAt)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
