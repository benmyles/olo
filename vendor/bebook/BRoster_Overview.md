<!-- System Overview - The Application Kit - BRoster -->

## BRoster

The [`BRoster`](BRoster.md "BRoster") object represents a service that keeps a roster of all applications currently running. It can provide information about any of those applications, activate one of them, add another application to the roster by launching it, or get information about an application to help you decide whether to launch it.

There's just one roster and it's shared by all applications. When an application starts up, a [`BRoster`](BRoster.md "BRoster") object is constructed and assigned to a global variable, [`be_roster`](BRoster.md#BRoster_be_roster "be_roster"). You always access the roster through this variable; you never have to instantiate a [`BRoster`](BRoster.md "BRoster") in application code.

The [`BRoster`](BRoster.md "BRoster") identifies applications in three ways:

  1. By entry_ref references to the executable files where they reside.

  2. By their signatures. The signature is a unique identifier for the application assigned as a file-system attribute or resource at compile time or by the [`BApplication`](BApplication.md "BApplication") constructor at run time.

  3. At run time, by their team_ids. A team is a group of threads sharing an address space; every application is a team.

If an application is launched more than once, the roster will include one entry for each instance of the application that's running. These instances will have the same signature, but different team identifiers.
