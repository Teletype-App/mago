<?php

/** @throws LengthException */
function replaced(): void
{
    try {
        throw new DomainException();
    } finally {
        throw new LengthException();
    }
}

function suppressed(): void
{
    try {
        throw new DomainException();
    } finally {
        return;
    }
}

/** @throws DomainException */
function preserved(): void
{
    try {
        throw new DomainException();
    } finally {
        $value = 1;
        unset($value);
    }
}

try {
    replaced();
    suppressed();
    preserved();
} catch (LogicException) {
}
