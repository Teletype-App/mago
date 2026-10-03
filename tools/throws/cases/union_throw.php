<?php
declare(strict_types=1);
namespace ThrowsProbe\union_throw;

function leaf(\DomainException|\LengthException $exception): void { throw $exception; }
function caller(\DomainException|\LengthException $exception): void { leaf($exception); }
