---- MODULE MC ----
\* Model values for UpdateSwap: the shipped package's top level, sorted as `names.sort()` sorts it.
\* NOTICE stands for a file the new package adds; the old folder holds the exe and tools.
EXTENDS UpdateSwap
MCOrder    == <<"NOTICE", "YTDugem.exe", "tools">>
MCDirs     == {"tools"}
MCExisting == {"YTDugem.exe", "tools"}
MCRequired == {"YTDugem.exe", "tools"}
====
