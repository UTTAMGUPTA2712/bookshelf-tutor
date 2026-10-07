// pdf.js setup. The viewer module reads `globalThis.pdfjsLib`, so it's loaded after that is set.
import * as pdfjsLib from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import "pdfjs-dist/web/pdf_viewer.css";

pdfjsLib.GlobalWorkerOptions.workerSrc = workerUrl;
(globalThis as unknown as { pdfjsLib: typeof pdfjsLib }).pdfjsLib = pdfjsLib;

export const viewerModule = import("pdfjs-dist/web/pdf_viewer.mjs");
export { pdfjsLib };
