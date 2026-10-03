<?php

function source(): void
{
    /** @mago-expect analysis:unhandled-thrown-type */
    throw new DomainException();
}

function middle(): void
{
    /** @mago-expect analysis:unhandled-thrown-type */
    source();
}

function caller(): void
{
    /** @mago-expect analysis:unhandled-thrown-type */
    middle();
}

function caught(): void
{
    try {
        caller();
    } catch (LogicException) {
    }
}

/** @throws DomainException */
function rethrown(): void
{
    try {
        caller();
    } catch (LogicException $e) {
        throw $e;
    }
}

try {
    caught();
    rethrown();
} catch (DomainException) {
}
