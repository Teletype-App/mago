<?php

interface Job
{
    public function run(): void;
}

final class FailingJob implements Job
{
    public function run(): void
    {
        /** @mago-expect analysis:unhandled-thrown-type */
        throw new DomainException();
    }
}

final class GoodJob implements Job
{
    public function run(): void {}
}

function dispatch(Job $job): void
{
    /** @mago-expect analysis:unhandled-thrown-type */
    $job->run();
}

function good(): void
{
    (new GoodJob())->run();
}

try {
    dispatch(new FailingJob());
    good();
} catch (DomainException) {
}
