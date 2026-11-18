# The EC driver queues query handlers without bound (a draft to send)

A draft of two reports about what happened on a ThinkPad P1 Gen 9 on
2026-11-17 (docs/AGENT-NOTES.md, "The embedded controller's event
storm"). Nothing has been sent. The first is for the kernel's ACPI
list (linux-acpi@vger.kernel.org), the second for Lenovo.

## For linux-acpi: ACPI: EC: a flood of one query starves all others

**What happened.** On a ThinkPad P1 Gen 9 (kernel 7.2.15, BIOS/EC
1.09), the embedded controller began reporting its AC adapter attached
and detached about 23 times a second (queries 0x26 and 0x27, a steady
42.9 ms period) and went on for two hours, charger in or out. Lenovo's
`_Q26`/`_Q27` each sleep 500 ms, then take a firmware mutex and sleep
100 ms more inside it, so however many workers `kec_query` has, about
ten of those methods finish a second. `acpi_ec_submit_query()` queued a
work item for every one of them: `queries_in_progress` reached 195,778.
Every other query of the controller's (the brightness keys, the USB-C
ports' UCSI notifications, the lid) waited behind them in the same
workqueue: none ran for two hours, and the load average read 270 from
blocked workers. After the controller
stopped, the backlog would have replayed for hours more; only a full
power-off ended it.

**Why.** `acpi_ec_submit_query()` allocates a query and queues it for
each event fetched, with no bound on how many are outstanding and no
coalescing of identical ones. `ec_max_queries` bounds how many run at
once, not how many wait. A query carries only its number; its handler
reads the state as it is when it runs, so of N identical queries queued
behind one that is running, N-1 say nothing new.

**A suggestion.** Coalesce identical pending queries: while a handler
for query value V is queued or running, a further V sets a "run again"
mark (or a small count) on it instead of queuing another work item.
The queue's length is then bounded by the number of distinct query
values, and one misbehaving event can't starve the rest. We run this
as an out-of-tree module that registers handlers through
`acpi_ec_add_query_handler()` in front of the `_Qxx` ones and counts in
that way (one running, up to eight owed); the same logic would be
simpler inside `ec.c`.

**To reproduce the mechanism without the hardware fault:** any EC
whose `_Qxx` sleeps (many do, under a mutex) and which can be made to
raise that query faster than the method returns.

## For Lenovo: ThinkPad P1 Gen 9, EC reports the adapter 23 times a second

- Machine: ThinkPad P1 Gen 9; BIOS/EC 1.09, PD firmware 1.04 at the
  time (since updated to bundle 1.17: BIOS/EC 1.10, PD 1.05).
- Sequence: running on an Apple Studio Display's USB-C power; the
  display unplugged; two minutes later the Lenovo 140 W charger plugged
  in, under load in a performance mode. Nine seconds after, EC RAM
  0x46 bit 4 (adapter present) began toggling every 42.9 ms and the EC
  raised queries 0x26/0x27 at that rate.
- It continued for two hours with the charger in, out, and in another
  port. The port held a 28 V, 5 A EPR contract throughout (UCSI
  GET_CONNECTOR_STATUS) with the sink path off; the battery supplied
  the machine and fell from 77% to 14%.
- Only a full power-off cleared it; it has not been seen since (once).
- Questions: is this a known EC issue fixed in 1.10; and can `_Q26` and
  `_Q27` avoid sleeping 600 ms each under MCPU, which is what turns a
  burst of adapter events into a backlog on any operating system that
  runs them one by one?
