<?php

/** @throws DomainException */
function conditional(bool $raise = false): void
{
    if ($raise) {
        throw new DomainException();
    }
}

function neverThrows(): void
{
    conditional(false);
    conditional();
    conditional(raise: false);
}

function throws(): void
{
    /** @mago-expect analysis:unhandled-thrown-type */
    conditional(true);
}

/** @throws DomainException */
function forwarded(bool $enabled): void
{
    conditional($enabled);
}

function forwardedFalse(): void
{
    forwarded(false);
}

function forwardedTrue(): void
{
    /** @mago-expect analysis:unhandled-thrown-type */
    forwarded(true);
}

/** @throws DomainException */
function reassigned(bool $raise): void
{
    $raise = true;
    conditional($raise);
}

function reassignedFalse(): void
{
    /** @mago-expect analysis:unhandled-thrown-type */
    reassigned(false);
}

try {
    neverThrows();
    throws();
    forwardedFalse();
    forwardedTrue();
    reassignedFalse();
} catch (DomainException) {
}
