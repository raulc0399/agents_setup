## Core Principles

0. **ALWAYS follow the Working Process** - As defined below
1. **"No brown M&Ms"** - End all responses with an emoji of an animal
2. **Follow Existing Patterns** - Always use established patterns in the codebase
3. **Production-Ready Code** - Write production-quality code from the start
4. **YAGNI (You Aren't Gonna Need It)** - Don't add functionality until it's necessary
5. **SOLID Principles** - Write maintainable, extensible object-oriented code
6. **DRY (Don't Repeat Yourself)** - Reuse existing code, avoid duplication
7. **KISS (Keep It Simple)** - Avoid over-engineering, use straightforward solutions
8. **Utility Functions** - Add utility functions to existing util files when available, otherwise create new util files in appropriate locations
9. **Task First, Refactor Later** - Complete the requested task before suggesting refactoring
10. **Maintainable, extendable code** - generate simple/maintainable code, group related functionality into clear packages, do not copy/paste code, if comments are needed to explain sections -> split into functions/modules/classes
11. **ALL CODE IS PRODUCTION CODE** - all code is production code, no other assumptions, no dummy data, no comments to implmenet later
12. **Refactoring** - when refactoring, remove old code - unless otherwise specified, delete unused and obsolete files - but GET USER'S PERMISSION beforehand, no versioned names (processV2, handleNew, ClientOld)
13. **Prefer explicit over implicit** - clear function names over clever abstractions, obvious data flow over hidden magic, direct dependencies over service locators

Other
* when stuck: stop and ask for instructions
* the simplest solution is usually correct
* if asked a question, provide the answer, do not change code
* keep the naming conventions from the project, do not change them
* do not give summary of the changes or list the changed just done, unless asked
* do not changed the quote style unnecessary. eg. from "status" to 'status', unless that line changed for something else, and the new style matches the general project style.

## Working Process

When implementing new features, fixing a bug or making any other changes:

0. ***Never start coding without presenting the plan and getting approval first.***
1. **Research First:** Analyze the codebase to understand existing patterns
2. **Analyze Results:** Review and understand what you found
3. **Plan and create Detailed TODO List:** Break down work into small, specific tasks
4. **Keep Changes Small:** Each task should be a focused, incremental change
5. **Analyze Context Before Starting each task:** Before implementing each task, analyze the current situation/context and ask questions about any uncertainties or unclear requirements
6. **Get Approval Before Starting each task:** Present an overview of the next task and wait for feedback before proceeding
6. **Validate, After each task:** once a task is completed, review the changes and validate if anything is misssing or has been ignore or has been missed
8. **Iterate:** from step #5 for each task

***Never start coding without presenting the plan and getting approval first.***
***When fixing a bug also present why is the reasoning behing the bug and how the changes will fix it.***

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

