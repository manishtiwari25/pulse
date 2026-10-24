const emptySummary = () => ({ code_deliveries: 0, code_files: 0, docs_only: 0, non_code: 0, unknown: 0, duplicates: 0 });

const validatePositiveInt = (raw, name, { max } = {}) => {
  if (raw === null || raw === "") return { value: name === "limit" ? 100 : 0 };
  if (!/^\d+$/.test(raw)) return { error: { status: 400, body: { error: "invalid " + name, why: name + " must be a positive whole number", fix: "Use a numeric " + name + "." } } };
  const value = Number(raw);
  if (!Number.isSafeInteger(value) || value <= 0) return { error: { status: 400, body: { error: "invalid " + name, why: name + " must be greater than zero", fix: "Use a positive " + name + "." } } };
  if (max && value > max) return { error: { status: 400, body: { error: "invalid " + name, why: name + " must be at most " + max, fix: "Use " + name + "=" + max + " or less." } } };
  return { value };
};

export function createCodeDeliveryMock(fixture, projects = []) {
  const records = [...(fixture.records || [])].sort((a, b) => Number(b.event_seq || 0) - Number(a.event_seq || 0));
  const knownProjects = new Set(projects.length ? projects : records.map((r) => r.project).filter(Boolean));
  const sparseWindows = fixture.sparse_windows || [];
  return {
    read(q) {
      const project = q.get("project") || "";
      if (project && !knownProjects.has(project)) return { status: 404, body: { error: "no such project", why: project + " is not registered in the mock store", fix: "Choose a registered project." } };
      const limitResult = validatePositiveInt(q.get("limit"), "limit", { max: 200 });
      if (limitResult.error) return limitResult.error;
      const beforeResult = validatePositiveInt(q.get("before"), "before");
      if (beforeResult.error) return beforeResult.error;
      const limit = limitResult.value;
      const before = beforeResult.value;
      const sparse = project ? sparseWindows.find((w) => w.project === project && Number(w.before || 0) === before) : null;
      if (sparse) {
        return {
          status: 200,
          body: {
            scope: { project: project || null, limit, scanned: 0, has_more: true, next_before: Number(sparse.next_before) },
            summary: emptySummary(),
            deliveries: [],
            unavailable: [],
          },
        };
      }
      const scoped = records.filter((r) => (!project || r.project === project) && (!before || Number(r.event_seq || 0) < before));
      const page = scoped.slice(0, limit);
      const more = scoped.length > limit;
      const summary = emptySummary();
      const deliveries = [];
      const unavailable = [];
      for (const row of page) {
        if (row.type === "delivery" && row.delivery) {
          summary.code_deliveries += 1;
          summary.code_files += Number(row.delivery.code_files || 0);
          deliveries.push(row.delivery);
        } else if (row.type === "docs_only") summary.docs_only += 1;
        else if (row.type === "non_code") summary.non_code += 1;
        else if (row.type === "duplicate") summary.duplicates += 1;
        else if (row.type === "unavailable") {
          summary.unknown += 1;
          unavailable.push({ event_seq: row.event_seq, project: row.project, at: row.at, reason: row.reason || "No trustworthy changed-file evidence" });
        }
      }
      const last = page[page.length - 1];
      return {
        status: 200,
        body: {
        scope: {
          project: project || null,
          limit,
          scanned: page.length,
          has_more: more,
          next_before: more && last ? Number(last.event_seq) : null,
        },
        summary,
        deliveries,
        unavailable,
        },
      };
    },
  };
}
