package runner

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

func completionQuestion(t *testing.T, s *store.Store, job row, opts postOpts) row {
	t.Helper()
	if opts.Kind == "" {
		opts.Kind = "question"
	}
	if opts.Project == "" {
		opts.Project = job.str("project")
	}
	if opts.Area == "" {
		opts.Area = "parser"
	}
	if opts.Thread == "" {
		opts.Thread = "empty-input"
	}
	if opts.Summary == "" {
		opts.Summary = "Why is empty input ignored?"
	}
	if opts.Agent == nil {
		opts.Agent = actorAgent(s, "requester")
	}
	if opts.Date == "" {
		opts.Date = plusHours(testNow, -1)
	}
	q, err := createPost(s, testNow, opts)
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET source = ? WHERE id = ?", "question:"+q.str("id"), job.str("id"))
	return q
}

func TestCompletionAnswerDeliveredToSource(t *testing.T) {
	skipSlow(t)
	for _, transport := range []string{"stdout", "finish-summary", "finish-raw"} {
		t.Run(transport, func(t *testing.T) {
			s, job, _, agent := newCompletionJob(t, "answer")
			q := completionQuestion(t, s, job, postOpts{})
			other := completionQuestion(t, s, job, postOpts{Date: plusMinutes(testNow, 1), Summary: "A later question still needs an answer."})
			exec1(s.DB, "UPDATE jobs SET source = ? WHERE id = ?", "question:"+q.str("id"), job.str("id"))
			text := "Empty input is ignored by the parser guard. " + strings.Repeat("The fixture documents this behavior. ", 8)
			text = strings.TrimSpace(text)
			status, result, output := "", "", "SUMMARY: "+text+"\n"
			if transport != "stdout" {
				status, result, output = "done", text, ""
				if transport == "finish-summary" {
					result = "SUMMARY: " + text
				}
			}
			r := runCompletion(t, s, job, agent, completionWorkerCommand(t, status, result, output, 0, ""))
			if r.str("status") != "done" {
				t.Fatalf("answer run: %s %q", r.str("status"), r.str("summary"))
			}
			answers := mustRows(s.DB, "SELECT * FROM posts WHERE kind = 'answer'")
			if len(answers) != 1 {
				t.Fatalf("successful answer produced %d answer post(s), want one", len(answers))
			}
			a := answers[0]
			if a.str("project") != "proj" || a.str("area") != q.str("area") || a.str("thread") != q.str("thread") ||
				a.str("on_post") != q.str("id") || a.str("agent") != agent || a.str("session") != r.str("id") ||
				a.str("summary") != trunc(text, 230) || strings.TrimSpace(a.str("body")) != text || a.str("resolution") != "" {
				t.Errorf("answer metadata/content mismatch: %v", a)
			}
			if postRow(s.DB, q.str("id")).str("resolution") != "" {
				t.Error("answer mutated the source question")
			}
			open := openQuestions(s, "proj", plusMinutes(testNow, 2))
			if len(open) != 1 || open[0].str("id") != other.str("id") {
				t.Errorf("source answer closed an unrelated question: %v", open)
			}
			if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'result' AND area = 'jobs' AND thread = ?", slugify(job.str("id"), 40)); n != 1 {
				t.Errorf("job result post count=%d, want one", n)
			}
			exec1(s.DB, "UPDATE jobs SET status = 'queued' WHERE id = ?", job.str("id"))
			runCompletion(t, s, job, agent, completionWorkerCommand(t, "", "", "SUMMARY: repeated delivery\n", 0, ""))
			if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'answer'"); n != 1 {
				t.Errorf("repeated completion duplicated answer: %d", n)
			}
		})
	}
}

func TestCompletionAnswerGuards(t *testing.T) {
	skipSlow(t)
	for _, name := range []string{
		"owner flag", "owner path", "owner path signal", "daemon question", "loop stop", "hard stop", "tier two", "closed thread", "jobs area",
		"missing source", "empty source", "malformed source", "wrong prefix", "wrong kind", "cross project",
		"failed", "cancelled", "cancelled finish", "question output", "question finish", "hard stop result", "empty summary", "non-answer job",
	} {
		t.Run(name, func(t *testing.T) {
			kind := "answer"
			if name == "non-answer job" {
				kind = "task"
			}
			s, job, _, agent := newCompletionJob(t, kind)
			opts := postOpts{}
			switch name {
			case "owner flag":
				opts.ForOwner = true
			case "owner path":
				opts.Body = "Should docs/evolution/STOP be removed?"
			case "owner path signal":
				opts.Body = "Signal: docs/evolution/constitution.md"
			case "daemon question":
				opts.Agent = actorAgent(s, "daemon")
			case "loop stop":
				opts.Summary = evolve.StopSummary
			case "hard stop":
				opts.Body = "Should the worker cross this hard stop?"
			case "tier two":
				opts.Summary = "Should ADR-021 be changed?"
			case "closed thread":
				opts.Resolution = "answered"
			case "jobs area":
				opts.Area = "jobs"
			case "wrong kind":
				opts.Kind = "finding"
			case "cross project":
				dir := t.TempDir()
				run(t, dir, "git", "init", "-q", "-b", "main")
				if _, _, err := addProject(s, dir, "Other", "", false, "", humanAgent, "other"); err != nil {
					t.Fatal(err)
				}
				opts.Project = "other"
			}
			q := completionQuestion(t, s, job, opts)
			source := "question:" + q.str("id")
			switch name {
			case "missing source":
				source = "question:missing"
			case "empty source":
				source = "question:"
			case "malformed source":
				source += ":extra"
			case "wrong prefix":
				source = "suggestion:" + q.str("id")
			}
			exec1(s.DB, "UPDATE jobs SET source = ? WHERE id = ?", source, job.str("id"))
			status, result, output, exit := "", "", "SUMMARY: investigated the parser\n", 0
			switch name {
			case "failed":
				exit = 1
			case "cancelled":
				exit = 5
			case "cancelled finish":
				status, result = "cancelled", "SUMMARY: stopped"
			case "question output":
				output += "QUESTION: Need an owner decision first.\n"
			case "question finish":
				status, result = "done", "SUMMARY: investigated\nQUESTION: Need an owner decision first."
			case "hard stop result":
				output = "SUMMARY: stopped at a hard stop; owner disposition required\n"
			case "empty summary":
				output = "FINDING: no conclusion yet\n"
			}
			runCompletion(t, s, job, agent, completionWorkerCommand(t, status, result, output, exit, ""))
			if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'answer'"); n != 0 {
				t.Errorf("guarded completion emitted %d answer(s)", n)
			}
		})
	}
}

func TestCompletionAnswerPreservesExistingAnswer(t *testing.T) {
	skipSlow(t)
	for _, mode := range []string{"linked", "thread", "same-time-thread", "worker-linked", "worker-thread"} {
		t.Run(mode, func(t *testing.T) {
			s, job, _, agent := newCompletionJob(t, "answer")
			q := completionQuestion(t, s, job, postOpts{})
			direct := ""
			if strings.HasPrefix(mode, "worker-") {
				direct = strings.TrimPrefix(mode, "worker-")
			} else {
				on, at := q.str("id"), testNow
				if mode != "linked" {
					on = ""
				}
				if mode == "same-time-thread" {
					at = q.str("date")
				}
				if _, err := createPost(s, at, postOpts{Kind: "answer", Project: "proj", Area: q.str("area"), Thread: q.str("thread"),
					Summary: "An independent answer.", Body: "Keep the independent answer.\n", On: on,
					Agent: actorAgent(s, "other-worker")}); err != nil {
					t.Fatal(err)
				}
			}

			runCompletion(t, s, job, agent, completionWorkerCommand(t, "", "", "SUMMARY: investigated the parser\n", 0, direct))
			answers := mustRows(s.DB, "SELECT * FROM posts WHERE kind = 'answer'")
			if len(answers) != 1 {
				t.Fatalf("existing answer duplicated: %d", len(answers))
			}
			if strings.Contains(answers[0].str("body"), "investigated the parser") {
				t.Fatal("runner replaced a direct or independent answer")
			}
		})
	}
}

func TestCompletionAnswerAfterOlderSameTimestampReply(t *testing.T) {
	s, job, _, agent := newCompletionJob(t, "answer")
	if _, err := createPost(s, testNow, postOpts{Kind: "answer", Project: "proj", Area: "parser", Thread: "empty-input",
		Summary: "An answer to an earlier question.", Agent: actorAgent(s, "other-worker"),
		Date: plusHours(testNow, -1)}); err != nil {
		t.Fatal(err)
	}
	q := completionQuestion(t, s, job, postOpts{})
	runCompletion(t, s, job, agent, completionWorkerCommand(t, "", "", "SUMMARY: The new question has a different answer.\n", 0, ""))
	if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'answer' AND on_post = ?", q.str("id")); n != 1 {
		t.Fatalf("an older answer with the same timestamp suppressed the new answer: %d linked answers", n)
	}
}

func TestCompletionQuestionTransportsDeduplicated(t *testing.T) {
	s, job, _, agent := newCompletionJob(t, "answer")
	completionQuestion(t, s, job, postOpts{})
	output := "SUMMARY: Investigated the parser.\nQUESTION: Need an owner decision first.\n"
	runCompletion(t, s, job, agent, completionWorkerCommand(t, "done", output, output, 0, ""))
	if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'question' AND area = 'jobs' AND thread = ?", slugify(job.str("id"), 40)); n != 1 {
		t.Fatalf("the same question from stdout and the job result was posted %d times", n)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'answer'"); n != 0 {
		t.Fatalf("a still-blocked answer emitted %d answers", n)
	}
}
