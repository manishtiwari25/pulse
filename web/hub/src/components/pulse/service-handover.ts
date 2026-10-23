import type { ServicePayload } from "@/lib/api";

export const HANDOVER_WAIT_MS = 45000;
const HANDOVER_POLL_MS = 1000;

export function waitForServiceHandover({ load, onReady, onLate }: { load: (signal: AbortSignal) => Promise<Pick<ServicePayload, "hub_is_service">>; onReady: () => void; onLate: (note: string) => void }): () => void {
  const controller = new AbortController();
  let finished = false;
  let timer: ReturnType<typeof setTimeout>;
  let note = "The service's console has not answered yet.";
  const stop = () => {
    finished = true;
    controller.abort();
    clearTimeout(timer);
    clearTimeout(deadline);
  };
  // A separate deadline also bounds a request that never answers.
  const deadline = setTimeout(() => {
    if (finished) return;
    stop();
    onLate(note);
  }, HANDOVER_WAIT_MS);

  const poll = () => {
    if (finished) return;
    void load(controller.signal).then(
      (status) => {
        if (finished) return;
        if (status.hub_is_service) {
          stop();
          onReady();
          return;
        }
        note = "The foreground console is still answering; the service has not taken over yet.";
        timer = setTimeout(poll, HANDOVER_POLL_MS);
      },
      (error: unknown) => {
        if (finished) return;
        note = error instanceof Error ? error.message : String(error);
        timer = setTimeout(poll, HANDOVER_POLL_MS);
      },
    );
  };
  // Give the foreground hub a moment to release its port before polling.
  timer = setTimeout(poll, 1500);
  return stop;
}
