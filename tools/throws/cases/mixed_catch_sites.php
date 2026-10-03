<?php
declare(strict_types=1);
namespace ThrowsProbe\mixed_catch_sites;

function leaf(): void { throw new \DomainException(); }
function caller(): void
{
    try { leaf(); } catch (\LogicException $e) {}
    leaf();
}
