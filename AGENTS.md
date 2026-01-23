## Core Principles

* when stuck: stop and ask for instructions
* the simplest solution is usually correct
* if asked a question, provide the answer, do not change code
* keep the naming conventions from the project, do not change them
* **do not give summary of the changes or list the changed just done**, unless asked
* keep changes minimal so the git diff is as small as possible. avoid unnecessary modifications, such as changing quote styles, formatting details or change/remove comments without semantic changes

0. **ALWAYS follow the Working Process** - As defined below
1. **"No brown M&Ms"** - End all responses with an emoji of a plane
2. **Follow Existing Patterns** - Always use established patterns in the codebase
3. **Avoid unnecessary complexity** - focus on readability
4. **Utility Functions** - if utility functions are needed, add to existing util files when available, otherwise create new util files in appropriate locations
5. **Task First, Refactor Later** - Complete the requested task before suggesting refactoring
6. **Maintainable, extendable code** - generate simple/maintainable code, group related functionality into clear packages, do not copy/paste code, if comments are needed to explain sections -> split into functions/modules/classes
7. **ALL CODE IS PRODUCTION CODE** - all code is production code, no other assumptions, no dummy data, no implement later comments or todos
8. **Refactoring** - when refactoring code, remove old code - unless otherwise specified, no versioned names (processV2, handleNew, ClientOld), delete unused and obsolete files - but GET USER'S PERMISSION before removing or deleting
9. **Prefer explicit over implicit** - clear function names over clever abstractions, obvious data flow over hidden magic, direct dependencies over service locators

## Working Process

When implementing new features, fixing a bug or making any other changes:

***Never start coding without presenting the plan and getting approval first.***
***When fixing a bug also present why is the reasoning behing the bug and how the changes will fix it.***

0. ***Never start coding without presenting the plan and getting approval first.***
1. **Research First:** Analyze the codebase to understand existing patterns
2. **Analyze Results:** Review and understand what you found
3. **Plan and create Detailed TODO List:** Break down work into small, specific tasks
4. **Keep Changes Small:** Each task should be a focused, incremental change
5. **Analyze Context Before Starting each task:** Before implementing each task, analyze the current situation/context and ask questions about any uncertainties or unclear requirements
6. **Get Approval Before Starting each task:** Present an overview of the next task and wait for feedback before proceeding
7. **Validate, After each task:** once a task is completed, review the changes and validate if anything is misssing or has been ignore or has been missed
8. **Iterate:** from step #5 for each task

## Language Conventions

- **Specifications, requests:** May be provided in German (client communication) or English
- **Conversation:** All discussions and responses in English
- **Code:**
  - All code elements must be in English
  - Variable names: English
  - Function/method names: English
  - Class names: English
  - Comments: English
  - Database column names: Follow existing conventions (mixed, prefer English for new)

