# Intended deviations A1-A8

The reimplementation follows the original program. It deviates only where the original depends on the frame rate or
on real time, is not deterministic, or is prone to crashes. These are the deliberate deviations A1-A8; the
identifiers appear in code comments.

| # | Original | Reimplementation |
|---|---|---|
| A1 | One `rand()` state serves simulation and visuals; the randomness of scripts depends on the particle count, the camera and `ParticlesQuality` | Simulation randomness has its own seed in the game state; visual randomness is separate |
| A2 | The tick clock drifts depending on the frame rate when `GameSpeed` != 10 (for example +1.6 % at 14) | Exact game time; "at most 2 ticks per frame, discard the rest" stays |
| A3 | The multiplayer checksum always adds 0 | A checksum is active for every tick |
| A4 | The per-frame update (slot 14) runs only for visible objects and writes positions | Pure presentation; the simulation never reads these fields. If it turns out that the original reads them, the behavior is moved into the tick deterministically |
| A5 | Single-player commands take effect at frame time, network commands at turn boundaries | Always at the tick boundary, as in network play |
| A6 | Camouflage (masking) stores the local player in the state | The simulation knows no local player; the host computes the fog view and combat points per viewer |
| A7 | Crash risks: path buffer of 1000 cells unchecked, a cycle in `guidgroups.ini`, an arrow with an empty effect list, unchecked memory errors | Limited or rejected with a log message; otherwise the behavior is as in the original |
| A8 | Construction and training check the building site, `REQUIRED_GUID` and costs only in the UI | The check is part of the command itself (commands arrive through MCP, the viewer and later the network) |

Further, individual deviations are marked in code comments as "deviation from the original".
