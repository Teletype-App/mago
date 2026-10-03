<?php

/** @param callable(): void $callback */
function invoke(callable $callback): void
{
    $callback();
}

/** @param callable(): void $callback */
function store(callable $callback): callable
{
    return $callback;
}

/** @param callable(): void $callback */
function handle(callable $callback): void
{
    try {
        $callback();
    } catch (LogicException) {
    }
}

function caller(): void
{
    /** @mago-expect analysis:unhandled-thrown-type */
    invoke(
        static function (): void {
            /** @mago-expect analysis:unhandled-thrown-type */
            throw new DomainException();
        },
    );
}

function handled(): void
{
    handle(
        static function (): void {
            /** @mago-expect analysis:unhandled-thrown-type */
            throw new DomainException();
        },
    );
}

function stored(): void
{
    $callback = store(
        static function (): void {
            /** @mago-expect analysis:unhandled-thrown-type */
            throw new DomainException();
        },
    );
    unset($callback);
}

function immediate(): void
{
    /** @mago-expect analysis:unhandled-thrown-type */
    (
        static function (): void {
            /** @mago-expect analysis:unhandled-thrown-type */
            throw new DomainException();
        }
    )();
}

try {
    caller();
    handled();
    stored();
    immediate();
} catch (DomainException) {
}
