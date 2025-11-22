## Core Principles

0. **"No brown M&Ms"** - End all responses with an emoji of an animal
1. **Follow Existing Patterns** - Always use established patterns in the codebase
2. **All Models Inherit from DB** - Every model class MUST extend the `DB` base class
3. **Production-Ready Code** - Write production-quality code from the start
4. **YAGNI (You Aren't Gonna Need It)** - Don't add functionality until it's necessary
5. **SOLID Principles** - Write maintainable, extensible object-oriented code
6. **DRY (Don't Repeat Yourself)** - Reuse existing code, avoid duplication
7. **KISS (Keep It Simple)** - Avoid over-engineering, use straightforward solutions
8. **Utility Functions** - Add utility functions to existing util files when available, otherwise create new util files in appropriate locations
9. **Task First, Refactor Later** - Complete the requested task before suggesting refactoring
10. **Maintainable, extendable code** - generate simple, maintainable, code, split into classes, modules, do not copy/paste code
11. **ALL CODE IS PRODUCTION CODE** - all code is production code, no other assumptions, no dummy data, no comments to implmenet later

## Working Process

When implementing new features or making changes:

1. **Research First:** Analyze the codebase to understand existing patterns
2. **Analyze Results:** Review and understand what you found
3. **Create Detailed TODO List:** Break down work into small, specific tasks
4. **Keep Changes Small:** Each task should be a focused, incremental change
5. **Analyze Context Before Starting:** Before implementing each task, analyze the current situation/context and ask questions about any uncertainties or unclear requirements
6. **Get Approval Before Starting:** Present a commit message of the last changes and an overview of the next task and wait for feedback before proceeding
7. **Iterate:** Repeat the approval process for each subsequent task

**Never start coding without presenting the plan and getting approval first.**

## Language Conventions

- **Specifications, requests:** May be provided in German (client communication) or English
- **Conversation:** All discussions and responses in English
- **Code:** All code elements must be in English
  - Variable names: English
  - Function/method names: English
  - Class names: English
  - Comments: English
  - Database column names: Follow existing conventions (mixed, prefer English for new)
