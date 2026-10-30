(() => {
  "use strict";

  const EXPECTED_SPOTCHECK_SIZE = 50;
  const CHOICES = [
    { value: "agree", label: "Agree" },
    { value: "disagree", label: "Disagree" },
    { value: "unsure", label: "Unsure" },
  ];
  const DIMENSION_LABELS = {
    activity: "Activity",
    failure_category: "Failure category",
    failure_detection: "Failure detection",
    retry_loop: "Retry / loop",
    anomaly: "Anomaly",
    phase: "Phase",
  };
  const REAL_DIMENSION_KEYS = ["activity", "failure_category", "retry_loop", "anomaly", "phase"];
  const FIELD_LABELS = {
    activity: "Activity",
    failure_category: "Failure category",
    retry_loop: "Retry / loop",
    anomaly: "Anomaly",
    phase: "Phase",
  };
  const PHASES = ["start", "active", "blocked", "recovery", "ending", "unknown"];
  const PHASE_COLORS = {
    start: "#b8d9cb",
    active: "#b8d8e9",
    blocked: "#efd5ae",
    recovery: "#d8c7e3",
    ending: "#b5d3c6",
    unknown: "#e5e9e6",
  };
  const CONDITION_COLORS = ["#0072b2", "#e69f00", "#009e73", "#d55e00", "#56b4e9", "#cc79a7", "#f0e442", "#000000", "#8a8a8a"];
  const CRITERIA = [
    { key: "anomaly_precision_lower95", title: "Anomaly precision", detail: "Lower 95% event-bootstrap bound; fewer than 10 positive events means insufficient support.", source: "anomaly_precision", unit: "ratio" },
    { key: "false_alerts_per_session_upper95", title: "False alerts per session", detail: "Upper 95% session-bootstrap bound on the mean false-positive alerts per complete session.", source: "false_alerts", unit: "count" },
    { key: "any_failure_recall_lower95", title: "Any-failure recall", detail: "Lower 95% event-bootstrap bound for detecting any failure.", source: "failure_recall", unit: "ratio" },
    { key: "retry_recall_lower95", title: "Retry recall", detail: "Lower 95% event-bootstrap bound for retry events.", source: "retry_recall", unit: "ratio" },
    { key: "loop_recall_lower95", title: "Loop recall", detail: "Lower 95% event-bootstrap bound for loop events.", source: "loop_recall", unit: "ratio" },
    { key: "activity_macro_f1_improvement_lower95", title: "Activity macro F1 improvement", detail: "Paired lower 95% event-bootstrap bound versus the rules baseline.", source: "improvement", unit: "difference" },
    { key: "provider_request_failure_rate", title: "Provider request failure rate", detail: "Observed failed requests divided by attempted requests. CI not estimated.", source: "request_failure_rate", unit: "ratio", operational: true },
    { key: "provider_http_p95_ms", title: "Provider HTTP P95", detail: "Observed provider request P95. CI not estimated.", source: "provider_p95", unit: "ms", operational: true },
    { key: "projected_async_p95_ms", title: "Projected async P95", detail: "Observed projected online P95. CI not estimated.", source: "projected_p95", unit: "ms", operational: true },
  ];
  const VIEW_NAMES = [
    ["criteria", "Acceptance"],
    ["tradeoff", "Quality & cost"],
    ["classes", "Per-class"],
    ["sessions", "Sessions"],
    ["failures", "Failure explorer"],
    ["methodology", "58 vs 436"],
    ["caveats", "Caveats"],
  ];

  const $ = (selector, root = document) => root.querySelector(selector);
  const make = (tag, className = "", text = null) => {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== null && text !== undefined) node.textContent = String(text);
    return node;
  };
  const clear = (node) => { while (node.firstChild) node.removeChild(node.firstChild); };
  const setVisible = (node, visible) => { node.hidden = !visible; };
  const safeJson = (value) => {
    if (typeof value === "string") return value;
    if (value === null || value === undefined) return "No context supplied.";
    try { return JSON.stringify(value, null, 2); } catch { return String(value); }
  };
  const humanize = (value) => String(value ?? "Unknown").replaceAll("_", " ").replaceAll("<abstain>", "abstain");
  const numeric = (value) => typeof value === "number" && Number.isFinite(value) ? value : null;
  const metricEstimate = (metric) => numeric(typeof metric === "object" && metric !== null ? metric.estimate : metric);
  const metricInterval = (metric, preferSession = false) => {
    if (!metric || typeof metric !== "object") return null;
    const candidates = preferSession
      ? [metric.session_bootstrap_ci95, metric.session_cluster_sensitivity_ci95, metric.event_bootstrap_ci95, metric.ci95]
      : [metric.event_bootstrap_ci95, metric.session_bootstrap_ci95, metric.session_cluster_sensitivity_ci95, metric.ci95];
    return candidates.find((ci) => ci && numeric(ci.lower) !== null && numeric(ci.upper) !== null) || null;
  };
  const formatNumber = (value, digits = 2) => {
    const number = numeric(value);
    if (number === null) return "Unavailable";
    return new Intl.NumberFormat(undefined, { maximumFractionDigits: digits, minimumFractionDigits: 0 }).format(number);
  };
  const formatPercent = (value, digits = 1) => {
    const number = numeric(value);
    return number === null ? "Unavailable" : `${formatNumber(number * 100, digits)}%`;
  };
  const formatUsd = (value) => {
    const number = numeric(value);
    return number === null ? "Unknown" : `$${number.toFixed(number < 0.01 ? 4 : 3)}`;
  };
  const formatMs = (value) => {
    const number = numeric(value);
    return number === null ? "Unavailable" : `${formatNumber(number, number < 10 ? 2 : 0)} ms`;
  };
  const shortText = (value, limit = 190) => {
    const text = safeJson(value).replace(/\s+/g, " ").trim();
    return text.length > limit ? `${text.slice(0, limit - 1)}…` : text;
  };
  const addOption = (select, value, label) => {
    const option = make("option", "", label);
    option.value = value;
    select.append(option);
  };
  const appendSectionHeading = (section, id, eyebrow, title, description) => {
    const header = make("div", "view-heading");
    const left = make("div");
    const label = make("p", "eyebrow", eyebrow);
    const heading = make("h3", "", title);
    heading.id = id;
    left.append(label, heading);
    header.append(left, make("p", "", description));
    section.append(header);
  };
  const addPanelTitle = (panel, title, note = "") => {
    const row = make("div", "panel-title-row");
    row.append(make("h4", "", title));
    if (note) row.append(make("p", "", note));
    panel.append(row);
  };
  const statusType = (status) => {
    const value = String(status || "unknown").toLowerCase();
    if (value === "pass" || value === "passed") return "pass";
    if (value === "fail" || value === "failed") return "fail";
    if (value.includes("insufficient") || value.includes("support")) return "insufficient";
    return "unknown";
  };
  const statusLabel = (status) => ({ pass: "Pass", fail: "Fail", insufficient: "Insufficient", unknown: "Not reported" })[statusType(status)];
  const addStatus = (parent, status, className = "status-pill") => {
    const kind = statusType(status);
    const node = make("span", `${className} status-${kind}`, statusLabel(status));
    parent.append(node);
    return node;
  };
  const conditionColor = (condition) => {
    const index = state.results?.conditions?.findIndex((item) => item.id === condition?.id) ?? -1;
    return CONDITION_COLORS[index >= 0 ? index % CONDITION_COLORS.length : 0];
  };
  const familyClass = (condition) => ["rules", "jev", "haiku"].includes(condition?.family) ? condition.family : "rules";
  const addFamilyMark = (parent, condition) => {
    const mark = make("span", `family-mark family-${familyClass(condition)}${condition?.context === "offline" ? " is-offline" : ""}`);
    mark.style.setProperty("--condition-color", conditionColor(condition));
    mark.setAttribute("aria-hidden", "true");
    parent.append(mark);
  };
  const conditionDimensions = (condition) => condition?.dimensions || condition?.metrics?.dimensions || condition?.[condition?.context]?.metrics?.dimensions || {};
  const dimension = (condition, key) => conditionDimensions(condition)?.[key] || {};
  const conditionLabel = (condition) => {
    const label = condition?.label || condition?.id || "Unnamed condition";
    const context = condition?.context ? humanize(condition.context) : "";
    return context && !label.toLowerCase().includes(context.toLowerCase()) ? `${label} · ${context}` : label;
  };
  const operationData = (condition) => condition?.operations || {};
  const normalizeCriteria = (condition) => condition?.acceptance?.criteria || {};
  const criteriaStatus = (condition) => condition?.acceptance?.status || "not_reported";
  const findCondition = (id) => state.results?.conditions?.find((condition) => condition.id === id) || null;
  const eventReference = (event) => event?.reference || event?.causal_reference || {};
  const predictionRows = (event, conditionId) => {
    const rows = event?.predictions?.[conditionId];
    return Array.isArray(rows) ? rows : [];
  };
  const labelValue = (labels, key) => labels && Object.prototype.hasOwnProperty.call(labels, key) ? labels[key] : null;
  const selector = (label, id, options, className = "select-field") => {
    const wrap = make("label", className);
    wrap.append(make("span", "", label));
    const select = make("select");
    select.id = id;
    options.forEach(([value, text]) => addOption(select, value, text));
    wrap.append(select);
    return { wrap, select };
  };

  const state = {
    review: null,
    events: [],
    answers: new Map(),
    results: null,
    currentIndex: 0,
    packetId: "",
    storageKey: "",
    revealKey: "",
    revealConfirmed: false,
    exportSignature: "",
    downloadGenerated: false,
    savedFileConfirmed: false,
    localStorageAvailable: true,
    revealed: false,
    importedStage: "blind",
    revealedViaImport: false,
    activeView: "criteria",
    selectedEvent: null,
  };

  function currentAnswersForExport(stage = "blind") {
    return state.events.flatMap((event) => {
      const answer = state.answers.get(event.id);
      if (!answer || !CHOICES.some((choice) => choice.value === answer.decision)) return [];
      return [{ packet_id: state.packetId, review_stage: stage, id: event.id, decision: answer.decision, note: answer.note || "" }];
    });
  }

  function draftSignature() {
    return JSON.stringify(currentAnswersForExport(state.importedStage));
  }

  function updateExportState() {
    const signature = draftSignature();
    if (state.exportSignature && state.exportSignature !== signature) {
      state.downloadGenerated = false;
      state.savedFileConfirmed = false;
      $("#saved-file-confirm").checked = false;
      setVisible($("#saved-file-confirm-wrap"), false);
      $("#export-status").textContent = "Choices changed since the last download. Download a fresh copy before revealing.";
    }
    renderReviewProgress();
    updateRevealButton();
  }

  function saveDraft() {
    if (!state.storageKey) return;
    const payload = {
      version: 1,
      packet_id: state.packetId,
      answers: state.events.flatMap((event) => {
        const answer = state.answers.get(event.id);
        return answer ? [{ id: event.id, decision: answer.decision, note: answer.note || "", review_stage: answer.review_stage || state.importedStage }] : [];
      }),
      stage: state.importedStage,
    };
    try {
      localStorage.setItem(state.storageKey, JSON.stringify(payload));
      $("#cache-status").textContent = `Draft saved in this browser · ${payload.answers.length} of ${state.events.length} choices`;
      state.localStorageAvailable = true;
    } catch {
      state.localStorageAvailable = false;
      $("#cache-status").textContent = "Browser cache is unavailable · download a review file to keep your work";
    }
  }

  function readDraft() {
    try {
      state.revealed = localStorage.getItem(state.revealKey) === "true";
      const raw = localStorage.getItem(state.storageKey);
      if (raw) {
        const payload = JSON.parse(raw);
        if (payload.packet_id === state.packetId && Array.isArray(payload.answers)) {
          state.importedStage = payload.stage === "post_reveal" ? "post_reveal" : "blind";
          const ids = new Set(state.events.map((event) => event.id));
          for (const answer of payload.answers) {
            if (!ids.has(answer.id) || !CHOICES.some((choice) => choice.value === answer.decision)) continue;
            state.answers.set(answer.id, {
              decision: answer.decision,
              note: typeof answer.note === "string" ? answer.note : "",
              review_stage: answer.review_stage === "post_reveal" ? "post_reveal" : state.importedStage,
            });
          }
        }
      }
      state.localStorageAvailable = true;
    } catch {
      state.localStorageAvailable = false;
      $("#cache-status").textContent = "Browser cache is unavailable · use the JSONL file to keep your work";
    }
    if (!state.localStorageAvailable) $("#cache-status").textContent = "Browser cache is unavailable · use the JSONL file to keep your work";
  }

  function renderReviewProgress() {
    const answered = state.events.reduce((count, event) => count + (state.answers.has(event.id) ? 1 : 0), 0);
    const countLabel = `${answered} / ${state.events.length}`;
    $("#review-count").textContent = countLabel;
    $("#review-progress").max = Math.max(state.events.length, 1);
    $("#review-progress").value = answered;
    $("#review-progress-copy").textContent = answered === state.events.length
      ? "Every event has a choice. Add a note only when it helps."
      : `${state.events.length - answered} event${state.events.length - answered === 1 ? "" : "s"} still need a choice.`;
    $("#spot-position").textContent = `Event ${state.currentIndex + 1} of ${state.events.length}`;
    $("#previous-event").disabled = state.currentIndex <= 0;
    $("#next-event").disabled = state.currentIndex >= state.events.length - 1;
    if (state.events.length !== EXPECTED_SPOTCHECK_SIZE) {
      $("#reveal-requirements").textContent = `This packet has ${state.events.length} events; exactly 50 are required for the blind spot-check.`;
    } else if (answered !== EXPECTED_SPOTCHECK_SIZE) {
      $("#reveal-requirements").textContent = `Complete all 50 choices (${answered} of 50 complete).`;
    } else if (!state.downloadGenerated || state.exportSignature !== draftSignature()) {
      $("#reveal-requirements").textContent = "Download a fresh JSONL file and confirm that you saved it.";
    } else if (!state.savedFileConfirmed) {
      $("#reveal-requirements").textContent = "Confirm that the downloaded JSONL file is saved somewhere you can find it.";
    } else if (!$("#reveal-ack").checked) {
      $("#reveal-requirements").textContent = "Acknowledge that the reveal cannot be made blind again in this browser.";
    } else {
      $("#reveal-requirements").textContent = "Your review file is confirmed. Reveal when you are ready.";
    }
  }

  function updateRevealButton() {
    const complete = state.events.length === EXPECTED_SPOTCHECK_SIZE && state.events.every((event) => state.answers.has(event.id));
    const exportCurrent = state.downloadGenerated && state.exportSignature === draftSignature();
    $("#reveal-results").disabled = state.revealed || !complete || !exportCurrent || !state.savedFileConfirmed || !$("#reveal-ack").checked;
  }

  function renderSpotcheck() {
    const event = state.events[state.currentIndex];
    if (!event) return;
    const ref = eventReference(event);
    $("#spot-event-title").textContent = `Event ${String(event.id ?? state.currentIndex + 1)}`;
    $("#spot-harness").textContent = humanize(event.harness || "Harness not supplied");
    $("#spot-context").textContent = safeJson(event.context);
    $("#spot-context-count").textContent = Array.isArray(event.context) ? `${event.context.length} context rows` : "sanitized";
    const reference = $("#spot-reference");
    clear(reference);
    const keys = ["activity", "failure_category", "retry_loop", "anomaly", "phase"];
    keys.forEach((key) => {
      const item = make("div");
      const value = ref[key];
      item.append(make("dt", "", FIELD_LABELS[key] || humanize(key)));
      const dd = make("dd", "", value === undefined || value === null ? "Not recorded" : humanize(value));
      if (value === true || value === false) dd.dataset.value = String(value);
      item.append(dd);
      reference.append(item);
    });
    const options = $("#decision-options");
    clear(options);
    const saved = state.answers.get(event.id);
    CHOICES.forEach((choice) => {
      const button = make("button", "decision-button", choice.label);
      button.type = "button";
      button.setAttribute("aria-pressed", String(saved?.decision === choice.value));
      button.disabled = state.revealed || state.importedStage === "post_reveal";
      button.addEventListener("click", () => {
        state.answers.set(event.id, { decision: choice.value, note: $("#spot-note").value, review_stage: state.importedStage });
        saveDraft();
        renderSpotcheck();
        updateExportState();
      });
      options.append(button);
    });
    $("#spot-note").value = saved?.note || "";
    $("#spot-note").disabled = state.revealed || state.importedStage === "post_reveal";
    $("#spot-note").oninput = () => {
      const current = state.answers.get(event.id);
      if (!current || state.revealed || state.importedStage === "post_reveal") return;
      current.note = $("#spot-note").value;
      state.answers.set(event.id, current);
      saveDraft();
      updateExportState();
    };
    renderReviewProgress();
  }

  function downloadReview() {
    if (!state.events.length || state.revealed) return;
    const rows = currentAnswersForExport(state.importedStage);
    if (!rows.length) {
      $("#export-status").textContent = "Choose at least one decision before downloading a review file.";
      return;
    }
    const content = `${rows.map((row) => JSON.stringify(row)).join("\n")}\n`;
    const blob = new Blob([content], { type: "application/x-ndjson;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const link = make("a");
    link.href = url;
    link.download = `asi-jev-review-${new Date().toISOString().replaceAll(":", "-")}.jsonl`;
    document.body.append(link);
    link.click();
    link.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
    state.exportSignature = draftSignature();
    state.downloadGenerated = true;
    state.savedFileConfirmed = false;
    $("#saved-file-confirm").checked = false;
    setVisible($("#saved-file-confirm-wrap"), true);
    $("#export-status").textContent = `${rows.length} choices sent to a browser download. The browser cannot verify the saved file or its location.`;
    updateExportState();
  }

  function validateImportRows(text) {
    const lines = text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
    if (!lines.length) throw new Error("The JSONL file has no review rows.");
    const knownIds = new Set(state.events.map((event) => event.id));
    const imported = [];
    const seen = new Set();
    let stage = null;
    for (let index = 0; index < lines.length; index += 1) {
      let row;
      try { row = JSON.parse(lines[index]); } catch { throw new Error(`Line ${index + 1} is not valid JSON.`); }
      const allowed = ["packet_id", "review_stage", "id", "decision", "note"];
      if (!row || typeof row !== "object" || Array.isArray(row) || Object.keys(row).some((key) => !allowed.includes(key)) || allowed.some((key) => !Object.prototype.hasOwnProperty.call(row, key))) {
        throw new Error(`Line ${index + 1} must contain only packet_id, review_stage, id, decision and note.`);
      }
      if (row.packet_id !== state.packetId) throw new Error(`Line ${index + 1} belongs to a different review packet.`);
      if (row.review_stage !== "blind" && row.review_stage !== "post_reveal") throw new Error(`Line ${index + 1} has an unknown review stage.`);
      if (stage && stage !== row.review_stage) throw new Error("The file mixes blind and post-reveal rows. Keep review stages in separate files.");
      stage = row.review_stage;
      if (!knownIds.has(row.id)) throw new Error(`Line ${index + 1} contains an unknown event ID.`);
      if (seen.has(row.id)) throw new Error(`The file contains duplicate event ID ${String(row.id)}.`);
      seen.add(row.id);
      if (!CHOICES.some((choice) => choice.value === row.decision)) throw new Error(`Line ${index + 1} has an invalid decision.`);
      if (typeof row.note !== "string") throw new Error(`Line ${index + 1} has a non-text note.`);
      if (row.note.length > 2000) throw new Error(`Line ${index + 1} has a note longer than 2,000 characters.`);
      imported.push({ id: row.id, decision: row.decision, note: row.note, review_stage: row.review_stage });
    }
    return { rows: imported, stage };
  }

  async function importReviewFile(file) {
    if (!file || state.revealed) return;
    try {
      if (file.size >= 1_000_000) throw new Error("Review files must be smaller than 1 MB.");
      const parsed = validateImportRows(await file.text());
      for (const row of parsed.rows) {
        state.answers.set(row.id, { decision: row.decision, note: row.note, review_stage: row.review_stage });
      }
      if (parsed.stage === "post_reveal") state.importedStage = "post_reveal";
      const unanswered = state.events.findIndex((event) => !state.answers.has(event.id));
      state.currentIndex = unanswered >= 0 ? unanswered : 0;
      saveDraft();
      renderSpotcheck();
      updateExportState();
      $("#review-status").textContent = parsed.stage === "post_reveal"
        ? `Imported ${parsed.rows.length} post-reveal choices. This review is not blind and editing is locked.`
        : `Imported ${parsed.rows.length} blind-review choices and merged them into the browser draft.`;
      if (parsed.stage === "post_reveal") {
        state.revealedViaImport = true;
        $("#review-status").textContent += " Export the stage-labeled file and acknowledge before opening this browser's results.";
      }
    } catch (error) {
      $("#review-status").textContent = error.message;
    }
  }

  async function fetchResults() {
    const response = await fetch("./results.json", { cache: "no-store" });
    if (!response.ok) throw new Error(`results.json returned ${response.status}`);
    const result = await response.json();
    if (!result || result.schema !== 1 || !Array.isArray(result.conditions)) throw new Error("results.json does not match schema 1.");
    state.results = result;
    renderResults();
    setVisible($("#loading-state"), false);
    setVisible($("#review-panel"), false);
    setVisible($("#results-app"), true);
  }

  async function revealResults() {
    if (state.revealed) return;
    state.revealConfirmed = true;
    $("#reveal-results").disabled = true;
    $("#review-status").textContent = "Opening the result file…";
    try {
      await fetchResults();
      state.revealed = true;
      state.importedStage = "post_reveal";
      try { localStorage.setItem(state.revealKey, "true"); } catch { state.localStorageAvailable = false; }
      $("#blindness-copy").textContent = state.localStorageAvailable
        ? "This browser remembers the reveal. The exported review remains a separate file captured before results appeared."
        : "This tab is revealed, but browser storage could not remember that state for a later visit.";
    } catch (error) {
      state.revealConfirmed = false;
      $("#review-status").textContent = `Results did not open: ${error.message}. The review remains locked; you can try again.`;
      updateRevealButton();
    }
  }

  async function revealAggregateOnly() {
    if (state.revealed || !$("#aggregate-reveal-ack").checked) return;
    $("#aggregate-reveal").disabled = true;
    $("#aggregate-status").textContent = "Opening the result file…";
    try {
      await fetchResults();
      state.revealed = true;
      try { localStorage.setItem(state.revealKey, "true"); } catch { state.localStorageAvailable = false; }
      $("#blindness-copy").textContent = state.localStorageAvailable
        ? "This browser remembers the reveal. There was no spot-check packet in this build."
        : "This tab is revealed, but browser storage could not remember that state for a later visit.";
    } catch (error) {
      $("#aggregate-status").textContent = `Results did not open: ${error.message}. You can try again.`;
      $("#aggregate-reveal").disabled = !$("#aggregate-reveal-ack").checked;
    }
  }

  function initReview(review) {
    if (!review || review.schema !== 1 || typeof review.packet_id !== "string" || typeof review.aggregate_only !== "boolean" || !Array.isArray(review.events)) {
      throw new Error("review.json does not match schema 1.");
    }
    state.review = review;
    state.events = review.events;
    if (!review.aggregate_only) {
      const eventIds = state.events.map((event) => event?.id);
      if (eventIds.some((id) => id === undefined || id === null) || new Set(eventIds).size !== eventIds.length) {
        throw new Error("review.json contains a missing or duplicate event ID.");
      }
    }
    state.packetId = review.packet_id;
    state.storageKey = `asi-jev-review-draft:${state.packetId}`;
    state.revealKey = `asi-jev-review-revealed:${state.packetId}`;
    readDraft();
    if (state.importedStage === "post_reveal") state.revealedViaImport = true;
    $("#download-review").addEventListener("click", downloadReview);
    $("#previous-event").addEventListener("click", () => { state.currentIndex = Math.max(0, state.currentIndex - 1); renderSpotcheck(); });
    $("#next-event").addEventListener("click", () => { state.currentIndex = Math.min(state.events.length - 1, state.currentIndex + 1); renderSpotcheck(); });
    $("#import-review").addEventListener("change", (event) => {
      const file = event.target.files?.[0];
      importReviewFile(file);
      event.target.value = "";
    });
    $("#saved-file-confirm").addEventListener("change", (event) => {
      state.savedFileConfirmed = event.target.checked;
      renderReviewProgress();
      updateRevealButton();
    });
    $("#reveal-ack").addEventListener("change", () => { renderReviewProgress(); updateRevealButton(); });
    $("#reveal-results").addEventListener("click", revealResults);
    $("#aggregate-reveal-ack").addEventListener("change", (changeEvent) => { $("#aggregate-reveal").disabled = !changeEvent.target.checked; });
    $("#aggregate-reveal").addEventListener("click", revealAggregateOnly);
    window.addEventListener("storage", (storageEvent) => {
      if (storageEvent.key !== state.revealKey || storageEvent.newValue !== "true" || state.revealed) return;
      state.revealed = true;
      state.importedStage = "post_reveal";
      $("#loading-state").textContent = "Results were revealed in another tab. Opening the result file…";
      setVisible($("#loading-state"), true);
      setVisible($("#review-panel"), false);
      fetchResults().then(() => {
        $("#blindness-copy").textContent = "Results were already revealed in another tab. Blind review cannot be restored.";
      }).catch((error) => showFatal(`Results were already revealed in another tab, but results.json could not be loaded: ${error.message}`));
    });

    if (review.aggregate_only) {
      setVisible($("#spotcheck-workspace"), false);
      setVisible($("#aggregate-review"), true);
    } else if (state.events.length === EXPECTED_SPOTCHECK_SIZE) {
      setVisible($("#spotcheck-workspace"), true);
      if (!state.events.some((item) => state.answers.has(item.id))) state.currentIndex = 0;
      else {
        const firstUnanswered = state.events.findIndex((item) => !state.answers.has(item.id));
        if (firstUnanswered >= 0) state.currentIndex = firstUnanswered;
      }
      renderSpotcheck();
      if (state.importedStage === "post_reveal") {
        $("#review-status").textContent = "This imported review is marked post-reveal. It is not blind; editing is locked.";
        $("#spotcheck-workspace").classList.add("is-post-reveal");
      }
      updateExportState();
    } else {
      setVisible($("#spotcheck-workspace"), false);
      const error = make("div", "notice notice-error", `This blind packet has ${state.events.length} events; exactly 50 are required. No results will load.`);
      $("#review-panel").append(error);
    }

    if (state.revealed) {
      $("#blindness-copy").textContent = "Results were already revealed in this browser. Blind review cannot be restored.";
      fetchResults().catch((error) => showFatal(`This browser already revealed the results, but results.json could not be loaded: ${error.message}`));
      return;
    }
    setVisible($("#loading-state"), false);
    setVisible($("#review-panel"), true);
    setVisible($("#results-app"), false);
    if (!state.localStorageAvailable) $("#review-status").textContent = "Browser cache is unavailable. Export the JSONL file to keep your work.";
  }

  function showFatal(message) {
    $("#load-error").querySelector("strong").textContent = state.revealed ? "Results were already revealed." : "Review packet could not be loaded.";
    $("#load-error-message").textContent = message;
    setVisible($("#load-error"), true);
    setVisible($("#loading-state"), false);
    setVisible($("#review-panel"), false);
  }

  function conditionMetric(condition, path) {
    const dims = conditionDimensions(condition);
    if (path === "anomaly_precision") return dims.anomaly?.per_class?.true?.precision || dims.anomaly?.true?.precision || dims.anomaly?.precision || null;
    if (path === "false_alerts") return condition.false_alert_budget?.mean_false_positive_alerts_per_session || condition.false_alert_budget?.mean || null;
    if (path === "failure_recall") return dims.failure_detection?.recall || dims.failure_detection?.per_class?.failure?.recall || dims.failure_category?.any_failure_recall || null;
    if (path === "retry_recall") return dims.retry_loop?.per_class?.retry?.recall || dims.retry_loop?.retry?.recall || dims.retry_loop?.retry_recall || null;
    if (path === "loop_recall") return dims.retry_loop?.per_class?.loop?.recall || dims.retry_loop?.loop?.recall || dims.retry_loop?.loop_recall || null;
    if (path === "improvement") return condition.improvement || null;
    return null;
  }

  function operationMetric(condition, source, criterion) {
    const ops = operationData(condition);
    const operationFields = ops.latency_ms || ops.latency || {};
    if (source === "request_failure_rate") {
      const explicit = ops.request_failure_rate || ops.request_failure_fraction || null;
      if (explicit) return explicit;
      const total = numeric(ops.request_count);
      const failures = numeric(ops.failed_request_count ?? ops.request_failure_count);
      if (total === null || total <= 0 || failures === null) return null;
      return { estimate: failures / total, ci_not_estimated: true };
    }
    if (source === "provider_p95") {
      const requestP95 = numeric(ops.request_rtt_ms?.p95_ms);
      if (requestP95 !== null) return { estimate: requestP95, ci_not_estimated: true };
    }
    const phase = condition.phase_boundary;
    let key = "";
    if (typeof phase === "string") key = phase;
    if (phase && typeof phase === "object") {
      const candidate = source === "provider_p95"
        ? phase.provider_http || phase.provider_http_p95 || phase.provider
        : phase.projected_async || phase.projected_online || phase.projected_async_p95;
      if (typeof candidate === "string") key = candidate;
      if (candidate && typeof candidate === "object") key = candidate.metric || candidate.key || "";
    }
    if (source === "provider_p95") key ||= "event_compute_or_http_ms";
    if (source === "projected_p95") key = "projected_online_latency_ms";
    const metric = operationFields[key];
    const p95 = metric && typeof metric === "object" ? numeric(metric.p95_ms ?? metric.p95) : null;
    if (p95 !== null) return { estimate: p95, ci_not_estimated: true };
    const fallback = source === "provider_p95"
      ? [ops.request_rtt_ms?.p95_ms, operationFields.event_compute_or_http_ms?.p95_ms]
      : [operationFields.projected_online_latency_ms?.p95_ms];
    const value = fallback.map(numeric).find((item) => item !== null);
    if (value !== undefined) return { estimate: value, ci_not_estimated: true };
    return null;
  }

  function getCriterionMetric(condition, meta, criterion) {
    if (meta.operational) return operationMetric(condition, meta.source, criterion);
    return conditionMetric(condition, meta.source);
  }

  function criterionDomain(metric, threshold, meta) {
    const estimate = metricEstimate(metric);
    const ci = metricInterval(metric, meta.source === "false_alerts");
    const values = [estimate, threshold, numeric(ci?.lower), numeric(ci?.upper)].filter((value) => value !== null);
    if (meta.unit === "ratio") return [0, 1];
    if (meta.unit === "difference") {
      const min = Math.min(-0.2, ...values);
      const max = Math.max(0.2, ...values);
      const padding = Math.max((max - min) * .1, .025);
      return [min - padding, max + padding];
    }
    const min = Math.min(0, ...values);
    const max = Math.max(1, ...values);
    return [min, max + Math.max(max * .12, .15)];
  }

  function svgNode(tag, attrs = {}, text = null) {
    const node = document.createElementNS("http://www.w3.org/2000/svg", tag);
    Object.entries(attrs).forEach(([key, value]) => node.setAttribute(key, String(value)));
    if (text !== null) node.textContent = String(text);
    return node;
  }

  function buildCriterionChart(metric, threshold, status, meta, decisionBound) {
    const svg = svgNode("svg", { viewBox: "0 0 410 54", class: "criterion-chart", role: "img", "aria-label": `${meta.title} estimate and confidence interval` });
    const x1 = 20;
    const x2 = 390;
    const y = 23;
    const [min, max] = criterionDomain(metric, threshold, meta);
    const point = metricEstimate(metric) ?? numeric(decisionBound);
    const ci = meta.operational ? null : metricInterval(metric, meta.source === "false_alerts");
    const scale = (value) => x1 + ((Math.min(max, Math.max(min, value)) - min) / Math.max(max - min, .000001)) * (x2 - x1);
    svg.append(svgNode("line", { x1, x2, y1: y, y2: y, class: "criterion-axis" }));
    if (numeric(threshold) !== null) svg.append(svgNode("line", { x1: scale(threshold), x2: scale(threshold), y1: 7, y2: 39, class: "criterion-threshold" }));
    if (ci) {
      const color = statusType(status) === "fail" ? "#d55e00" : "#56b4e9";
      svg.append(svgNode("line", { x1: scale(ci.lower), x2: scale(ci.upper), y1: y, y2: y, class: "criterion-ci", stroke: color }));
      svg.append(svgNode("line", { x1: scale(ci.lower), x2: scale(ci.lower), y1: 18, y2: 28, class: "criterion-cap", stroke: color }));
      svg.append(svgNode("line", { x1: scale(ci.upper), x2: scale(ci.upper), y1: 18, y2: 28, class: "criterion-cap", stroke: color }));
    }
    if (point !== null) {
      const color = statusType(status) === "pass" ? "#009e73" : statusType(status) === "fail" ? "#d55e00" : "#667575";
      let mark;
      if (statusType(status) === "pass") mark = svgNode("circle", { cx: scale(point), cy: y, r: 6, fill: color, class: "criterion-dot" });
      else if (statusType(status) === "fail") mark = svgNode("rect", { x: scale(point) - 5.5, y: y - 5.5, width: 11, height: 11, fill: color, class: "criterion-dot" });
      else mark = svgNode("path", { d: `M ${scale(point)} ${y - 7} L ${scale(point) + 7} ${y} L ${scale(point)} ${y + 7} L ${scale(point) - 7} ${y} Z`, fill: color, class: "criterion-dot" });
      const title = svgNode("title", {}, `${meta.title}: ${formatCriterionValue(point, meta.unit)}${ci ? `, 95% CI ${formatCriterionValue(ci.lower, meta.unit)} to ${formatCriterionValue(ci.upper, meta.unit)}` : ", CI not estimated"}`);
      mark.append(title);
      svg.append(mark);
    }
    svg.append(svgNode("text", { x: x1, y: 49, class: "criterion-scale" }, formatCriterionValue(min, meta.unit)));
    svg.append(svgNode("text", { x: x2, y: 49, class: "criterion-scale", "text-anchor": "end" }, formatCriterionValue(max, meta.unit)));
    return svg;
  }

  function formatCriterionValue(value, unit) {
    if (numeric(value) === null) return "Unavailable";
    if (unit === "ratio") return formatPercent(value, 0);
    if (unit === "difference") return `${value > 0 ? "+" : ""}${formatNumber(value, 2)}`;
    if (unit === "ms") return `${formatNumber(value, 0)} ms`;
    return formatNumber(value, 2);
  }

  function makeConditionSelector(parent, id, selected = "") {
    const options = state.results.conditions.map((condition) => [condition.id, conditionLabel(condition)]);
    const control = selector("Condition", id, options);
    if (selected) control.select.value = selected;
    parent.append(control.wrap);
    return control.select;
  }

  function renderResults() {
    const results = state.results;
    $("#incomplete-banner").hidden = results.complete !== false;
    const counts = results.counts || {};
    const eventCount = numeric(counts.events);
    const sessionCount = numeric(counts.sessions);
    $("#result-counts").textContent = `${eventCount === null ? "Events not reported" : `${eventCount} scored events`} · ${sessionCount === null ? "sessions not reported" : `${sessionCount} sessions`}`;
    buildNavigation();
    renderCriteriaView();
    renderTradeoffView();
    renderClassesView();
    renderSessionsView();
    renderFailureView();
    renderMethodologyView();
    renderCaveatsView();
    showView(state.activeView);
  }

  function buildNavigation() {
    const nav = $("#results-nav");
    clear(nav);
    VIEW_NAMES.forEach(([id, label]) => {
      const button = make("button", "nav-button", label);
      button.type = "button";
      button.dataset.view = id;
      button.setAttribute("aria-current", String(state.activeView === id ? "page" : "false"));
      button.addEventListener("click", () => showView(id));
      nav.append(button);
    });
  }

  function showView(id) {
    state.activeView = id;
    VIEW_NAMES.forEach(([view]) => {
      const section = $(`#view-${view}`);
      if (section) section.hidden = view !== id;
    });
    $$(".nav-button").forEach((button) => button.setAttribute("aria-current", button.dataset.view === id ? "page" : "false"));
  }

  function $$(selector, root = document) { return Array.from(root.querySelectorAll(selector)); }

  function renderCriteriaView() {
    const section = $("#view-criteria");
    clear(section);
    appendSectionHeading(section, "title-criteria", "01 · ACCEPTANCE", "Does any condition qualify?", "The frozen gates are shown as recorded. The chart uses the underlying metric and its measured 95% interval where one exists.");
    const overview = make("div", "panel");
    addPanelTitle(overview, "Condition acceptance", "Each row summarizes the frozen gate result.");
    const list = make("div", "condition-list");
    state.results.conditions.forEach((condition) => {
      const row = make("div", "condition-row");
      const name = make("div", "condition-name");
      addFamilyMark(name, condition);
      name.append(make("span", "condition-name-text", conditionLabel(condition)));
      row.append(name, make("span", "context-chip", humanize(condition.family || "family unknown")));
      addStatus(row, criteriaStatus(condition));
      list.append(row);
    });
    overview.append(list);
    const selectorPanel = make("div", "panel");
    const selectorRow = make("div", "control-row");
    const defaultCondition = state.results.conditions.find((item) => item.acceptance)?.id || state.results.conditions[0]?.id || "";
    const conditionSelect = makeConditionSelector(selectorRow, "criteria-condition", defaultCondition);
    selectorPanel.append(selectorRow);
    const charts = make("div", "criterion-grid");
    const renderSelected = () => {
      clear(charts);
      const condition = findCondition(conditionSelect.value);
      if (!condition) { charts.append(make("div", "empty-state", "No condition data is available.")); return; }
      const acceptance = condition.acceptance || {};
      const criteria = normalizeCriteria(condition);
      CRITERIA.forEach((meta) => {
        const criterion = criteria[meta.key] || {};
        const metric = getCriterionMetric(condition, meta, criterion);
        const point = metricEstimate(metric);
        const ci = meta.operational ? null : metricInterval(metric, meta.source === "false_alerts");
        const card = make("div", "criterion-card");
        const copy = make("div", "criterion-copy");
        copy.append(make("h5", "", meta.title), make("p", "", meta.detail));
        const chart = buildCriterionChart(metric, criterion.threshold, criterion.status, meta, criterion.bound);
        const result = make("div", "criterion-result");
        const displayValue = point !== null ? formatCriterionValue(point, meta.unit) : criterion.bound !== undefined ? `Bound ${formatCriterionValue(criterion.bound, meta.unit)}` : "No estimate";
        result.append(make("strong", "", displayValue));
        if (ci) result.append(make("span", "", `95% CI ${formatCriterionValue(ci.lower, meta.unit)}–${formatCriterionValue(ci.upper, meta.unit)}`));
        else result.append(make("span", "", meta.operational ? "CI not estimated" : "95% CI not available"));
        if (criterion.unique_positive_events !== undefined) {
          const positive = numeric(criterion.unique_positive_events);
          const minimum = numeric(criterion.minimum_unique_positive_events) ?? 10;
          result.append(make("span", positive !== null && positive < minimum ? "low-support" : "", `${formatNumber(positive, 0)} positive events${positive !== null && positive < minimum ? ` · fewer than ${formatNumber(minimum, 0)} needed` : ""}`));
        }
        if (criterion.unique_positive_events_by_class && typeof criterion.unique_positive_events_by_class === "object") {
          const minimum = numeric(criterion.minimum_unique_positive_events) ?? 10;
          const lowClasses = Object.entries(criterion.unique_positive_events_by_class).filter(([, count]) => numeric(count) !== null && count < minimum);
          if (lowClasses.length) result.append(make("span", "low-support", `Below ${formatNumber(minimum, 0)}: ${lowClasses.map(([label, count]) => `${humanize(label)} (${formatNumber(count, 0)})`).join(", ")}`));
        }
        if (numeric(criterion.bound) !== null) result.append(make("span", "", `Decision bound ${formatCriterionValue(criterion.bound, meta.unit)}`));
        result.append(make("span", "", `Threshold ${formatCriterionValue(criterion.threshold, meta.unit)}`));
        addStatus(result, criterion.status || acceptance.status, "status-text");
        card.append(copy, chart, result);
        charts.append(card);
      });
      const key = make("div", "criterion-key");
      key.append(make("span", "key-item", ""));
      const ciKey = make("span", "key-item");
      ciKey.append(make("i", "key-ci"), document.createTextNode(" 95% interval"));
      const thresholdKey = make("span", "key-item");
      thresholdKey.append(make("i", "key-line"), document.createTextNode(" Threshold"));
      const dotKey = make("span", "key-item");
      dotKey.append(make("i", "key-dot"), document.createTextNode(" Estimate"));
      key.replaceChildren(dotKey, ciKey, thresholdKey);
      charts.append(key);
    };
    conditionSelect.addEventListener("change", renderSelected);
    renderSelected();
    selectorPanel.append(charts);
    section.append(overview, selectorPanel);
  }

  function getActivityMetric(condition, name) {
    return dimension(condition, "activity")?.[name] || null;
  }

  function conditionLatency(condition) {
    const ops = operationData(condition);
    const latency = ops.latency_ms || ops.latency || {};
    if (condition.family === "rules") {
      const rulesP95 = numeric(latency.event_compute_or_http_ms?.p95_ms);
      return rulesP95 ?? numeric(latency.projected_online_latency_ms?.p95_ms);
    }
    const requestP95 = numeric(ops.request_rtt_ms?.p95_ms);
    return requestP95 ?? numeric(latency.event_compute_or_http_ms?.p95_ms);
  }

  function conditionCost(condition) {
    const ops = operationData(condition);
    const candidates = [
      [ops.cost_usd_estimate, ops.cost_status],
      [ops.cost_usd, ops.cost_status],
      [ops.jev_list_cost_usd_estimate, ops.jev_list_cost_status],
      [ops.haiku_billed_cost_usd, ops.haiku_billed_cost_status],
      [ops.cost?.estimate ?? ops.cost?.amount, ops.cost?.status],
    ];
    for (const [raw, status] of candidates) {
      const value = numeric(raw);
      if (value !== null && !["unknown", "not_applicable"].includes(status)) return { value, status: status || "reported" };
    }
    const status = candidates.map((candidate) => candidate[1]).find(Boolean) || "unknown";
    if (status === "not_applicable" && condition.family === "rules") return { value: null, status: "no provider billing" };
    return { value: null, status: "unknown" };
  }

  function addSvgFamilyPoint(svg, condition, x, y, titleText, radius = 7) {
    const color = conditionColor(condition);
    let marker;
    const family = familyClass(condition);
    const offline = condition.context === "offline";
    const markerClass = `plot-point${offline ? " is-offline" : ""}`;
    const fill = offline ? "#ffffff" : color;
    if (family === "rules") marker = svgNode("circle", { cx: x, cy: y, r: radius, fill, class: markerClass });
    else if (family === "jev") marker = svgNode("path", { d: `M ${x} ${y - radius} L ${x + radius} ${y} L ${x} ${y + radius} L ${x - radius} ${y} Z`, fill, class: markerClass });
    else marker = svgNode("rect", { x: x - radius, y: y - radius, width: radius * 2, height: radius * 2, rx: 1, fill, class: markerClass });
    marker.style.setProperty("--condition-color", color);
    marker.append(svgNode("title", {}, titleText));
    svg.append(marker);
  }

  function renderTradeoffView() {
    const section = $("#view-tradeoff");
    clear(section);
    appendSectionHeading(section, "title-tradeoff", "02 · QUALITY / OPERATIONS", "Quality comes with a runtime shape.", "Compare activity macro F1 with observed P95 latency. The cost panel keeps unknown and non-applicable values explicit.");
    const layout = make("div", "tradeoff-layout");
    const plotPanel = make("div", "panel");
    addPanelTitle(plotPanel, "Activity macro F1 vs P95 latency", "One point per condition · shape and color identify family.");
    const wrap = make("div", "plot-wrap");
    const svg = svgNode("svg", { viewBox: "0 0 620 350", class: "scatter-svg", role: "img", "aria-label": "Scatter plot of activity macro F1 against P95 latency" });
    const left = 62, right = 595, top = 20, bottom = 292;
    const latencies = state.results.conditions.map(conditionLatency).filter((value) => value !== null);
    const maxLatency = Math.max(1500, 10000, ...latencies) * 1.08;
    const px = (value) => left + Math.max(0, Math.min(1, value)) * (right - left);
    const py = (value) => bottom - Math.max(0, Math.min(maxLatency, value)) / maxLatency * (bottom - top);
    [0, .25, .5, .75, 1].forEach((tick) => {
      const x = px(tick);
      svg.append(svgNode("line", { x1: x, x2: x, y1: top, y2: bottom, class: "plot-grid" }));
      svg.append(svgNode("text", { x, y: bottom + 19, class: "plot-label", "text-anchor": "middle" }, formatPercent(tick, 0)));
    });
    [0, 2500, 5000, 7500, 10000, 15000].filter((tick) => tick <= maxLatency).forEach((tick) => {
      const y = py(tick);
      svg.append(svgNode("line", { x1: left, x2: right, y1: y, y2: y, class: "plot-grid" }));
      svg.append(svgNode("text", { x: left - 8, y: y + 3, class: "plot-label", "text-anchor": "end" }, formatNumber(tick, 0)));
    });
    [1500, 10000].forEach((target) => {
      if (target <= maxLatency) {
        const y = py(target);
        svg.append(svgNode("line", { x1: left, x2: right, y1: y, y2: y, class: "plot-target" }));
        svg.append(svgNode("text", { x: right - 3, y: y - 4, class: "plot-label", "text-anchor": "end" }, `${formatNumber(target, 0)} ms target`));
      }
    });
    svg.append(svgNode("text", { x: (left + right) / 2, y: 340, class: "plot-axis-label", "text-anchor": "middle" }, "Activity macro F1 (higher is better)"));
    const yTitle = svgNode("text", { x: 15, y: (top + bottom) / 2, class: "plot-axis-label", transform: `rotate(-90 15 ${(top + bottom) / 2})`, "text-anchor": "middle" }, "Provider HTTP / rules compute P95 (ms · lower is faster)");
    svg.append(yTitle);
    state.results.conditions.forEach((condition) => {
      const quality = metricEstimate(getActivityMetric(condition, "macro_f1"));
      const latency = conditionLatency(condition);
      if (quality === null || latency === null) return;
      addSvgFamilyPoint(svg, condition, px(quality), py(latency), `${conditionLabel(condition)} · activity macro F1 ${formatPercent(quality)} · P95 ${formatMs(latency)}`);
    });
    wrap.append(svg);
    const legend = make("div", "legend-row");
    state.results.conditions.forEach((condition) => {
      const item = make("span", "legend-condition");
      addFamilyMark(item, condition);
      item.append(document.createTextNode(conditionLabel(condition)));
      legend.append(item);
    });
    plotPanel.append(wrap, legend, make("p", "tiny-note", "Provider points use pooled request RTT P95; rules points use local compute P95. The 1,500 ms Jev and 10,000 ms Haiku lines are provider targets; the projected async P95 gate is separate and appears in Acceptance."));

    const costPanel = make("div", "panel");
    addPanelTitle(costPanel, "Reported cost per condition", "Dollar values may be estimates, not invoices.");
    const costs = state.results.conditions.map((condition) => ({ condition, ...conditionCost(condition) }));
    const known = costs.map((row) => row.value).filter((value) => value !== null);
    const maxCost = Math.max(...known, .000001);
    const costList = make("div", "cost-list");
    costs.forEach((row) => {
      const item = make("div", "cost-row");
      const name = make("span", "cost-name", conditionLabel(row.condition));
      const track = make("span", `cost-track${row.condition.context === "offline" ? " is-offline" : ""}`);
      track.style.setProperty("--condition-color", conditionColor(row.condition));
      if (row.value !== null) {
        const point = make("i", `cost-point${row.condition.context === "offline" ? " is-offline" : ""}`);
        point.style.left = `${Math.max(0, Math.min(100, row.value / maxCost * 100))}%`;
        track.append(point);
      }
      const statusText = humanize(row.status);
      const valueText = row.value === null ? statusText : `${formatUsd(row.value)} · ${statusText}`;
      const value = make("span", row.value === null ? "cost-unknown" : "cost-value", valueText);
      item.append(name, track, value);
      costList.append(item);
    });
    costPanel.append(costList, make("p", "tiny-note", "Unknown cost is shown as unknown; the chart does not treat it as $0. Rules-only conditions with no provider billing are marked not applicable."));
    layout.append(plotPanel, costPanel);
    section.append(layout);
  }

  function renderClassesView() {
    const section = $("#view-classes");
    clear(section);
    appendSectionHeading(section, "title-classes", "03 · LABEL DETAIL", "See where each label holds.", "Precision and recall intervals are event-bootstrap intervals. Small positive classes are flagged because their estimates are unstable.");
    const panel = make("div", "panel");
    const controls = make("div", "control-row");
    const conditionSelect = makeConditionSelector(controls, "classes-condition", state.results.conditions[0]?.id || "");
    const dimensionSelectControl = selector("Dimension", "classes-dimension", []);
    controls.append(dimensionSelectControl.wrap);
    panel.append(controls);
    const layout = make("div", "class-layout");
    const classPanel = make("div", "panel");
    const heatPanel = make("div", "panel");
    layout.append(classPanel, heatPanel);
    panel.append(layout);
    const render = () => {
      const condition = findCondition(conditionSelect.value);
      const dimensions = conditionDimensions(condition);
      const dimensionKeys = Object.keys(dimensions);
      const oldDimension = dimensionSelectControl.select.value;
      clear(dimensionSelectControl.select);
      dimensionKeys.forEach((key) => addOption(dimensionSelectControl.select, key, DIMENSION_LABELS[key] || humanize(key)));
      if (dimensionKeys.includes(oldDimension)) dimensionSelectControl.select.value = oldDimension;
      renderPerClass(classPanel, condition, dimensionSelectControl.select.value);
      renderHeatmap(heatPanel, condition, dimensionSelectControl.select.value);
    };
    conditionSelect.addEventListener("change", render);
    dimensionSelectControl.select.addEventListener("change", render);
    render();
    section.append(panel);
  }

  function renderPerClass(panel, condition, dimensionKey) {
    clear(panel);
    const dim = dimension(condition, dimensionKey);
    addPanelTitle(panel, `${DIMENSION_LABELS[dimensionKey] || humanize(dimensionKey)} · per-class`, conditionLabel(condition));
    const classes = dim?.per_class || {};
    const entries = Object.entries(classes);
    if (!entries.length) { panel.append(make("div", "empty-state", "Per-class estimates are not present in this build.")); return; }
    const wrap = make("div", "table-wrap");
    const table = make("table", "data-table");
    const head = make("thead");
    const headerRow = make("tr");
    ["Class", "Positive events", "Precision", "Recall"].forEach((text) => headerRow.append(make("th", "", text)));
    head.append(headerRow);
    const body = make("tbody");
    entries.forEach(([label, item]) => {
      const row = make("tr");
      row.append(make("td", "class-label", humanize(label)));
      const positive = numeric(item.positive_events);
      const supportCell = make("td", positive !== null && positive < 10 ? "low-support" : "", positive === null ? "—" : formatNumber(positive, 0));
      row.append(supportCell);
      row.append(metricCell(item.precision), metricCell(item.recall));
      body.append(row);
    });
    table.append(head, body);
    wrap.append(table);
    panel.append(wrap, make("p", "tiny-note", "Values are mean-over-repetitions event-level estimates. Fewer than 10 unique positive events is insufficient evidence for primary-class acceptance."));
  }

  function metricCell(metric) {
    const cell = make("td");
    const estimate = metricEstimate(metric);
    cell.append(make("span", "", estimate === null ? "—" : formatPercent(estimate)));
    const ci = metricInterval(metric);
    if (ci) cell.append(make("span", "ci-value", `${formatPercent(ci.lower)}–${formatPercent(ci.upper)} · 95%`));
    else cell.append(make("span", "ci-value", "95% CI unavailable"));
    return cell;
  }

  function renderHeatmap(panel, condition, dimensionKey) {
    clear(panel);
    addPanelTitle(panel, "Confusion heatmap", "Rows are references · columns are predictions.");
    const data = state.results.confusion?.[condition?.id]?.[dimensionKey];
    if (!data || !Array.isArray(data.labels) || !Array.isArray(data.matrix)) {
      panel.append(make("div", "empty-state", "Confusion counts are not present for this condition and dimension."));
      return;
    }
    const rowLabels = Array.isArray(data.row_labels) ? data.row_labels : data.labels;
    const max = Math.max(1, ...data.matrix.flat().map((value) => numeric(value) ?? 0));
    const grid = make("div", "table-wrap");
    const heat = make("div", "heatmap-grid");
    const columns = `82px repeat(${data.labels.length}, 34px)`;
    const headRow = make("div", "heatmap-row");
    headRow.style.gridTemplateColumns = columns;
    headRow.append(make("div", "heatmap-cell is-label", "Reference"));
    data.labels.forEach((label) => headRow.append(make("div", "heatmap-cell is-head", humanize(label))));
    heat.append(headRow);
    data.matrix.forEach((row, rowIndex) => {
      const line = make("div", "heatmap-row");
      line.style.gridTemplateColumns = columns;
      line.append(make("div", "heatmap-cell is-label", humanize(rowLabels[rowIndex] ?? `Row ${rowIndex + 1}`)));
      row.forEach((value, columnIndex) => {
        const count = numeric(value) ?? 0;
        const alpha = .06 + (count / max) * .72;
        const isCorrect = rowLabels[rowIndex] === data.labels[columnIndex];
        const cell = make("div", `heatmap-cell${isCorrect ? " is-correct" : ""}`, formatNumber(count, 2));
        cell.style.backgroundColor = `${isCorrect ? "rgba(0, 158, 115" : "rgba(0, 114, 178"}, ${alpha.toFixed(3)})`;
        cell.title = `${formatNumber(count, 2)} mean predictions${isCorrect ? " · reference and prediction labels match" : ""}`;
        line.append(cell);
      });
      heat.append(line);
    });
    grid.append(heat);
    panel.append(grid, make("p", "heatmap-note", "Darker cells contain higher mean counts across repetitions. Cells with matching reference and prediction labels have a green outline; other cells show disagreements."));
  }

  function renderSessionsView() {
    const section = $("#view-sessions");
    clear(section);
    appendSectionHeading(section, "title-sessions", "04 · SESSION SEQUENCE", "Follow the phase changes.", "Select a captured session. Each row shows the reference or one condition across event order; click any event to inspect its context and labels.");
    const panel = make("div", "panel");
    const controls = make("div", "control-row session-controls");
    const sessions = Array.isArray(state.results.sessions) ? state.results.sessions : [];
    const sessionControl = selector("Session", "session-select", sessions.map((session) => [session.key, `${session.key} · ${humanize(session.harness || "harness unknown")}`]));
    const trialControl = selector("Prediction trial", "session-trial", [["1", "Trial 1"], ["2", "Trial 2"], ["3", "Trial 3"]]);
    controls.append(sessionControl.wrap, trialControl.wrap);
    const trialNote = make("p", "tiny-note", "Choose trial 1, 2 or 3. Rules conditions use their sole saved trial for each view and label that source trial explicitly.");
    panel.append(controls, trialNote);
    panel.append(controls);
    const phaseLegend = make("div", "phase-legend");
    PHASES.forEach((phase) => {
      const item = make("span", "phase-key");
      const swatch = make("i", "phase-swatch");
      swatch.style.setProperty("--phase-color", PHASE_COLORS[phase]);
      item.append(swatch, document.createTextNode(humanize(phase)));
      phaseLegend.append(item);
    });
    panel.append(phaseLegend);
    const timelineScroll = make("div", "timeline-scroll");
    const timeline = make("div", "timeline");
    timelineScroll.append(timeline);
    panel.append(timelineScroll);
    const markerLegend = make("div", "marker-legend");
    [["anomaly", "Anomaly"], ["retry", "Retry"], ["loop", "Loop"], ["failure", "Failure"]].forEach(([kind, label]) => {
      const item = make("span");
      item.append(make("i", `marker marker-${kind}`), document.createTextNode(` ${label}`));
      markerLegend.append(item);
    });
    panel.append(markerLegend);
    const redraw = () => drawSessionTimeline(timeline, sessions.find((session) => session.key === sessionControl.select.value), Number(trialControl.select.value));
    sessionControl.select.addEventListener("change", redraw);
    trialControl.select.addEventListener("change", redraw);
    redraw();
    section.append(panel);
  }

  function markersFor(labels) {
    const markers = [];
    if (labels?.anomaly === true || labels?.anomaly === "true") markers.push("anomaly");
    const retry = String(labels?.retry_loop || "").toLowerCase();
    if (retry === "retry") markers.push("retry");
    if (retry === "loop") markers.push("loop");
    const failureCategory = String(labels?.failure_category || "").toLowerCase();
    if (labels?.activity === "failure" || labels?.failure_detection === true || (failureCategory && failureCategory !== "none")) markers.push("failure");
    return markers;
  }

  function drawSessionTimeline(root, session, requestedTrial = 1) {
    clear(root);
    if (!session || !Array.isArray(session.events) || !session.events.length) {
      root.append(make("div", "timeline-empty", "No session sequence is available in this results build."));
      return;
    }
    const events = [...session.events].sort((a, b) => (numeric(a.order) ?? 0) - (numeric(b.order) ?? 0));
    const referenceRow = make("div", "timeline-row");
    const label = make("div", "timeline-label", "Frozen reference");
    referenceRow.append(label);
    const refTrack = make("div", "timeline-track");
    events.forEach((event, index) => refTrack.append(timelineCell(event, session, "reference", index + 1, eventReference(event))));
    referenceRow.append(refTrack);
    root.append(referenceRow);
    state.results.conditions.forEach((condition) => {
      const row = make("div", "timeline-row");
      const rowLabel = make("div", "timeline-label");
      addFamilyMark(rowLabel, condition);
      const available = session.events.map((event) => predictionRows(event, condition.id)).find((rows) => rows.length) || [];
      const exact = available.find((item) => Number(item.repetition ?? 1) === requestedTrial);
      const reused = !exact && available.length === 1;
      const shownTrial = exact ? requestedTrial : reused ? Number(available[0].repetition ?? 1) : requestedTrial;
      const soleRulesTrial = condition.family === "rules" && available.length === 1;
      rowLabel.append(make("span", "", `${conditionLabel(condition)} · trial ${shownTrial}${soleRulesTrial ? " · sole rules trial" : reused ? " reused" : ""}`));
      const track = make("div", "timeline-track");
      events.forEach((event, index) => {
        const rows = predictionRows(event, condition.id);
        const selected = rows.find((item) => Number(item.repetition ?? 1) === requestedTrial) || (rows.length === 1 ? rows[0] : null);
        const labels = {};
        ["activity", "failure_category", "retry_loop", "anomaly", "phase"].forEach((key) => {
          labels[key] = selected ? labelValue(selected.labels, key) : null;
        });
        const cell = timelineCell(event, session, condition.id, index + 1, labels, condition.context);
        const actualTrial = selected?.repetition ?? (rows.length === 1 ? rows[0].repetition ?? 1 : requestedTrial);
        const reuseText = rows.length === 1 && requestedTrial !== Number(actualTrial) ? " reused (sole saved trial)" : "";
        cell.title = `Event ${numeric(event.order) ?? index + 1} · trial ${actualTrial}${reuseText} · ${humanize(labels.phase || "unknown")} · open context`;
        cell.setAttribute("aria-label", `${session.key}, event ${numeric(event.order) ?? index + 1}, ${conditionLabel(condition)}, trial ${actualTrial}${reuseText}, phase ${humanize(labels.phase || "unknown")}. Open context.`);
        track.append(cell);
      });
      row.append(rowLabel, track);
      root.append(row);
    });
  }

  function timelineCell(event, session, conditionId, displayOrder, labels, context = "") {
    const phase = String(labels?.phase || "unknown").toLowerCase();
    const button = make("button", `timeline-cell${context === "offline" ? " is-offline" : ""}`);
    button.type = "button";
    button.style.setProperty("--phase-color", PHASE_COLORS[phase] || PHASE_COLORS.unknown);
    const order = numeric(event.order) ?? displayOrder;
    button.setAttribute("aria-label", `${session.key}, event ${order}, ${conditionId === "reference" ? "reference" : conditionLabel(findCondition(conditionId))}, phase ${humanize(phase)}. Open context.`);
    button.title = `Event ${order} · ${humanize(phase)} · open context`;
    button.append(make("span", "event-index", formatNumber(order, 0)));
    const flags = make("span", "timeline-markers");
    markersFor(labels).forEach((marker) => flags.append(make("i", `marker marker-${marker}`)));
    button.append(flags);
    button.addEventListener("click", () => openEventDialog(session, event));
    return button;
  }

  function openEventDialog(session, event) {
    const dialog = $("#event-dialog");
    $("#dialog-title").textContent = `${session.key} · event ${String(event.order ?? event.id ?? "")}`;
    const body = $("#dialog-body");
    clear(body);
    const contextSection = make("section", "dialog-section");
    contextSection.append(make("h3", "", "Sanitized event context"), make("pre", "", safeJson(event.context)));
    body.append(contextSection);
    const refSection = make("section", "dialog-section");
    refSection.append(make("h3", "", "Frozen reference"), make("pre", "", safeJson(eventReference(event))));
    body.append(refSection);
    if (event.causal_reference && event.causal_reference !== event.reference) {
      const causalSection = make("section", "dialog-section");
      causalSection.append(make("h3", "", "Causal reference"), make("pre", "", safeJson(event.causal_reference)));
      body.append(causalSection);
    }
    const predictionsSection = make("section", "dialog-section");
    predictionsSection.append(make("h3", "", "Every repetition by condition"));
    const grid = make("div", "dialog-prediction-grid");
    state.results.conditions.forEach((condition) => {
      const card = make("div", "prediction-card");
      card.append(make("strong", "", conditionLabel(condition)));
      const rows = predictionRows(event, condition.id);
      if (!rows.length) card.append(make("span", "", "Prediction not present"));
      rows.forEach((row, index) => card.append(make("span", "", `R${row.repetition ?? index + 1}: ${safeJson(row.labels)}`)));
      grid.append(card);
    });
    predictionsSection.append(grid);
    body.append(predictionsSection);
    if (typeof dialog.showModal === "function") dialog.showModal();
  }

  function renderFailureView() {
    const section = $("#view-failures");
    clear(section);
    appendSectionHeading(section, "title-failures", "05 · DISAGREEMENTS", "Inspect the misses and false alarms.", "Filters run across every scored event and every saved repetition. The context opens as sanitized text; no model output is hidden behind a summary.");
    const panel = make("div", "panel");
    const controls = make("div", "control-row");
    const conditionControl = selector("Condition", "failure-condition", [["__all__", "All conditions"], ...state.results.conditions.map((condition) => [condition.id, conditionLabel(condition)])]);
    conditionControl.select.value = "__all__";
    const conditionSelect = conditionControl.select;
    const dimensionControl = selector("Dimension", "failure-dimension", []);
    const classControl = selector("Class", "failure-class", []);
    controls.append(conditionControl.wrap, dimensionControl.wrap, classControl.wrap);
    const summary = make("p", "failure-summary");
    const list = make("div", "failure-list");
    panel.append(controls, summary, list);
    section.append(panel);
    const render = () => {
      const oldDimension = dimensionControl.select.value;
      const selectedConditions = conditionSelect.value === "__all__" ? state.results.conditions : [findCondition(conditionSelect.value)].filter(Boolean);
      const condition = selectedConditions[0] || null;
      const dimensions = conditionDimensions(condition);
      clear(dimensionControl.select);
      REAL_DIMENSION_KEYS.filter((key) => Object.prototype.hasOwnProperty.call(dimensions, key)).forEach((key) => addOption(dimensionControl.select, key, DIMENSION_LABELS[key] || humanize(key)));
      if (Object.prototype.hasOwnProperty.call(dimensions, oldDimension)) dimensionControl.select.value = oldDimension;
      const dimensionKey = dimensionControl.select.value;
      const classes = new Set(selectedConditions.flatMap((item) => state.results.confusion?.[item.id]?.[dimensionKey]?.labels || []));
      const events = (state.results.sessions || []).flatMap((session) => (session.events || []).map((event) => ({ session, event })));
      events.forEach(({ event }) => {
        const ref = eventReference(event)[dimensionKey];
        if (ref !== undefined && ref !== null) classes.add(String(ref));
        selectedConditions.forEach((item) => predictionRows(event, item.id).forEach((prediction) => {
          const value = labelValue(prediction.labels, dimensionKey);
          if (value !== undefined && value !== null) classes.add(String(value));
        }));
      });
      const priorClass = classControl.select.value;
      clear(classControl.select);
      addOption(classControl.select, "__all__", "Any class");
      [...classes].sort((a, b) => a.localeCompare(b)).forEach((value) => addOption(classControl.select, value, humanize(value)));
      if ([...classControl.select.options].some((option) => option.value === priorClass)) classControl.select.value = priorClass;
      drawFailureRows(list, summary, selectedConditions, dimensionKey, classControl.select.value, events);
    };
    conditionSelect.addEventListener("change", render);
    dimensionControl.select.addEventListener("change", render);
    classControl.select.addEventListener("change", render);
    render();
  }

  function drawFailureRows(list, summary, conditions, dimensionKey, classFilter, eventItems) {
    clear(list);
    const failures = [];
    eventItems.forEach(({ session, event }) => {
      const reference = eventReference(event)[dimensionKey];
      const values = conditions.flatMap((condition) => predictionRows(event, condition.id).map((row, index) => ({ condition: conditionLabel(condition), repetition: row.repetition ?? index + 1, value: labelValue(row.labels, dimensionKey) })));
      if (!values.length) return;
      const relevant = classFilter === "__all__" || String(reference) === classFilter || values.some((item) => String(item.value) === classFilter);
      const mismatch = reference !== undefined && reference !== null && values.some((item) => item.value === undefined || item.value === null || String(item.value) !== String(reference));
      if (relevant && mismatch) failures.push({ session, event, reference, values });
    });
    const conditionSummary = conditions.length === state.results.conditions.length ? "all conditions" : conditionLabel(conditions[0]);
    summary.textContent = `${failures.length} event${failures.length === 1 ? "" : "s"} with a disagreement · ${conditionSummary} · ${DIMENSION_LABELS[dimensionKey] || humanize(dimensionKey)} · every available trial checked`;
    if (!failures.length) { list.append(make("div", "empty-state", "No mismatches match these filters, or no predictions are available.")); return; }
    failures.forEach(({ session, event, reference, values }) => {
      const row = make("article", "failure-row");
      row.append(make("span", "failure-id", `${session.key} · #${event.order ?? event.id ?? "?"}`));
      const actual = make("div", "failure-values");
      actual.append(make("strong", "", `Reference: ${humanize(reference)}`));
      values.forEach((item) => {
        const value = item.value === undefined || item.value === null ? "missing" : humanize(item.value);
        actual.append(make("span", "", `${item.condition} · R${item.repetition}: ${value}`));
      });
      const context = make("span", "failure-context", shortText(event.context));
      const open = make("button", "button button-quiet failure-open", "Open context");
      open.type = "button";
      open.addEventListener("click", () => openEventDialog(session, event));
      row.append(actual, context, open);
      list.append(row);
    });
  }

  function renderMethodologyView() {
    const section = $("#view-methodology");
    clear(section);
    appendSectionHeading(section, "title-methodology", "06 · COHORTS", "58 fixture events beside 436 real-session events.", "The first experiment checked feasibility. The second replayed a purposive, error-enriched local cohort. These samples answer different questions.");
    const first = state.results.first_run || {};
    const counts = state.results.counts || {};
    const firstEvents = numeric(first.events);
    const firstSessions = numeric(first.sessions);
    const realEvents = numeric(counts.events);
    const realSessions = numeric(counts.sessions);
    const grid = make("div", "method-grid");
    grid.append(methodCard("FIRST PASS · FIXTURES", "Feasibility benchmark", "30 mock native-format fixtures + 28 authored challenges. These are not real human sessions.", [
      ["Events", firstEvents], ["Fixture sessions", firstSessions],
    ], "fixture"));
    grid.append(methodCard("REAL-SESSION STUDY", "Local CLI replay", "Eligible events from complete captured physical sessions. Cohort roles are confounded, and selection enriches for errors.", [
      ["Events", realEvents], ["Physical sessions", realSessions],
    ], "real"));

    const tablePanel = make("div", "panel first-run-table");
    addPanelTitle(tablePanel, "Activity metrics side by side", "Accuracy and macro F1 are shown only when the source provides them.");
    const tableWrap = make("div", "table-wrap");
    const table = make("table", "data-table");
    const head = make("thead");
    const hr = make("tr");
    ["Condition", "Fixture accuracy", "Fixture macro F1", "Real accuracy", "Real macro F1"].forEach((label) => hr.append(make("th", "", label)));
    head.append(hr);
    const body = make("tbody");
    (first.conditions || []).forEach((fixture) => {
      const real = findCondition(fixture.id);
      const acc = metricEstimate(getActivityMetric(real, "accuracy"));
      const f1 = metricEstimate(getActivityMetric(real, "macro_f1"));
      const row = make("tr");
      row.append(make("td", "class-label", real ? conditionLabel(real) : fixture.label || fixture.id || "Condition"));
      row.append(make("td", "", fixture.activity_accuracy === undefined ? "—" : formatPercent(fixture.activity_accuracy)));
      row.append(make("td", "", fixture.activity_macro_f1 === undefined ? "—" : formatPercent(fixture.activity_macro_f1)));
      row.append(make("td", "", acc === null ? "—" : formatPercent(acc)));
      row.append(make("td", "", f1 === null ? "—" : formatPercent(f1)));
      body.append(row);
    });
    table.append(head, body);
    tableWrap.append(table);
    tablePanel.append(tableWrap);
    const note = make("div", "comparison-note", "Do not read this as a before/after score on the same population. The 58-event set is made from mock-format fixtures and authored challenges; the 436-event set is from 20 local captured sessions. Sampling, context and label evidence differ.");
    section.append(grid, tablePanel, note);
  }

  function methodCard(kicker, title, description, metrics, kind) {
    const card = make("article", "method-card");
    const label = make("div", "method-kicker");
    label.append(make("span", "", kicker), make("span", "tag", kind === "fixture" ? "Synthetic / authored" : "Captured cohort"));
    card.append(label, make("h4", "", title), make("p", "", description));
    const values = make("div", "method-metrics");
    metrics.forEach(([labelText, value]) => {
      const item = make("div", "method-metric");
      item.append(make("span", "", labelText), make("strong", "", value === null ? "—" : formatNumber(value, 0)));
      values.append(item);
    });
    card.append(values);
    return card;
  }

  function renderCaveatsView() {
    const section = $("#view-caveats");
    clear(section);
    appendSectionHeading(section, "title-caveats", "07 · HOW TO READ THIS", "Useful evidence, with boundaries.", "These results describe one frozen cohort and one scoring reference. They do not establish general deployment performance.");
    const conclusion = make("div", "overall-conclusion");
    conclusion.append(make("p", "eyebrow", "STUDY CONCLUSION"), make("h4", "", "The measured conditions did not qualify for production ASI integration."), make("p", "", "The evaluation report recommends a separate asynchronous shadow experiment as the next step. No production parser, runtime or storage contract is changed by this result viewer."));
    const caveats = [
      ["The reference is not human gold", "Two retained blind AI labellers and a separate AI adjudicator used Sol at high reasoning. They share a model family, so correlated annotation errors remain possible."],
      ["Harness and role are confounded", "The cohort contains 12 Claude Manager captures and eight Codex Worker captures. It cannot separate a harness effect from a role effect."],
      ["One context boundary differs from protocol prose", "The frozen formatter takes an older context tail by bytes, which can start in the middle of a record. The protocol describes selecting whole rows; read the context framing with that difference in mind."],
      ["The cohort is selected", "These 436 events come from 20 complete captures and distinct tasks. Sampling was purposive and enriched for errors; it does not represent everyday prevalence."],
      ["A complete capture is not a finished task", "The native file was complete when captured. That does not prove the original task ended or that a terminal event was present."],
      ["Intervals have a narrow scope", "Bootstrap intervals measure event or session resampling around this reference. They do not include label uncertainty, cohort selection bias or future workload changes."],
      ["Repetitions do not add events", "Three model repetitions are averaged within each event. They measure run-to-run stability; they do not turn 436 events into 1,308 independent samples."],
      ["Rare classes have little support", "The cohort has 44 failure, 10 retry, four loop and four anomaly positives. Activity has zero wait, five recovery, two completion and eight unknown positives. The primary-class rule requires at least 10 unique positives."],
      ["Alert rates are cohort-specific", "The false-alert budget is calculated on this enriched sample. It should not be read as the expected alert count in normal production use."],
      ["Cost can remain unknown", "Gateway bills are unknown. Direct Jev list-price amounts are estimates, not invoices. Unknown values stay unknown."],
      ["Latency boundaries differ", "Provider request time, caller compute, queue wait and projected online latency are separate measures. Operational P95 and request-failure figures are descriptive; CI was not estimated."],
      ["Your spot-check does not rewrite the reference", "Agree, disagree, unsure and notes are exported as a separate review file. They do not change or correct the frozen AI reference."],
    ];
    const grid = make("div", "caveat-grid");
    caveats.forEach(([title, detail], index) => {
      const card = make("article", "caveat-card");
      card.append(make("span", "caveat-number", String(index + 1).padStart(2, "0")), make("h4", "", title), make("p", "", detail));
      grid.append(card);
    });
    section.append(conclusion, grid);
  }

  $("#dialog-close").addEventListener("click", () => $("#event-dialog").close());
  $("#event-dialog").addEventListener("click", (event) => {
    if (event.target === $("#event-dialog")) $("#event-dialog").close();
  });

  fetch("./review.json", { cache: "no-store" })
    .then((response) => {
      if (!response.ok) throw new Error(`review.json returned ${response.status}`);
      return response.json();
    })
    .then(initReview)
    .catch((error) => showFatal(error.message));
})();
