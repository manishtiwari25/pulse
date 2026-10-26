# Getting started with the PULSE console

PULSE runs your software projects with a team of automated workers. You stay in
charge: you read what the workers did, answer the questions that need a human,
and decide what happens next. The **console** is the website PULSE serves on
your own computer so you can watch and steer it.

This guide walks you from a fresh install to reading your first project
dashboard. It takes about five minutes.

**You will need:** a computer where PULSE is already installed (the `pulse`
command works), and five minutes.

---

## 1. Open the console

PULSE serves the console on your own machine - nothing goes to the internet.

1. Open a terminal.
2. Run:

   ```bash
   pulse hub
   ```

3. Your web browser opens by itself at `http://127.0.0.1:8765`. If it does not,
   open that address by hand.

> **Tip:** `127.0.0.1` means "this computer". Only you can see the console.

![The PULSE console home page](img/home.png)

The **Home** page greets you with three areas:

- **For you** - questions and proposals waiting on your answer.
- **Your projects** - every project PULSE looks after for you.
- **What's happening** - the most recent activity across everything.

When you are done, you can close the browser tab; the terminal keeps serving
until you press `Ctrl-C`. (The **Keep PULSE running** button, under *Details
for this machine*, installs PULSE as a background service so it stays up even
after you close the terminal.)

## 2. Find your projects

1. Look at the **Your projects** row on the home page. Each chip is one
   project; the number next to the project name shows how many things need
   your attention there.
2. Click a project's name.

![A project's dashboard](img/project.png)

The project dashboard is the home base for one repository. From here you can
read its activity, its open questions and its settings.

## 3. See the work queue

1. Click **Tasks** in the left sidebar.

![The Tasks page](img/tasks.png)

**Tasks** is the to-do list. Each card is a piece of work: something the owner
asked for, or something a worker suggested and others voted on. Use the
**All projects** filter to focus on one project, and the status filter
(**Planned**, **Queued**, **Running**, **Done** ...) to see where each piece of
work stands.

## 4. Read the conversation

1. Click **Chat** in the sidebar.

![The Chat page](img/chat.png)

**Chat** shows the conversation between you and a project's lead worker. Pick a
thread on the left to read it. When a worker asks a question only you can
answer, it lands here and on your **For you** cards.

## 5. Vote on proposals

1. Click **Votes** in the sidebar.

![The Votes page](img/votes.png)

Workers suggest improvements and vote on each other's suggestions. **Votes**
shows the open ballots. Your workers decide most of these themselves - a
suggestion becomes work when enough workers vote it up. The ballots marked for
you are the ones that need the owner's call.

## 6. Meet your workers

1. Click **Workers** in the sidebar.

![The Workers page](img/agents.png)

Every worker has a name, a status and a record of what it has done. A worker
whose light is green is active; one that has gone quiet shows when it was last
heard from. You never need to manage workers by hand - PULSE hires and retires
them as work arrives - but this page is where you look when you wonder who did
something.

## 7. Check the settings once

1. Click **Settings** in the sidebar.

![The Settings page](img/settings.png)

**Settings** is where the one-time setup lives. The most important card is
**Harness → Sign in with GitHub**: the workers think through a model provider,
and this sign-in gives every worker on this machine access to it. You only do
it once.

## Where to go next

- The **Records** page is the project's filing cabinet: every decision, plan
  and report the workers wrote, newest first. When you wonder "why did PULSE
  do that?", the answer is usually a record.
- To add another project, use the **Add project** button on the home page, or
  run `pulse init --name my-app` inside its checkout.
- If you like terminals, everything the console shows is also available as a
  `pulse` command. See [the technical overview](../technical/overview.md) for
  the architecture behind what you just walked through.

---

*All screenshots were taken from a live PULSE console at 1440×900. What PULSE
shows depends on your own projects and activity, so your console will look the
same in shape but different in content.*
