# mem-analyser

**WIP**

A tool to analyse memory usage on a microcontroller.

## Features

- flash bin before monitoring
- use obj file compiled from either rust or cpp source code
- include asm file to use functionality and get more info on monitored point
- different analyse modes
- write monitored information to json file

## Usage

Get all tool options by executing:

```Bash
mem-analyser --help
```

## Modes

- stepping: User can step over every instruction. (Difficult when having interrupts)
- looping: Monitors every defined interval.
- single-shot: Run to defined point and get monitoring data.
- loop-measure: WIP